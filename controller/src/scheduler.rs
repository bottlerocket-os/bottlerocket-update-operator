use chrono::{DateTime, Utc};
use cron::Schedule;
use lazy_static::lazy_static;
use regex::Regex;
use snafu::{OptionExt, ResultExt};
use std::env;
use std::str::FromStr;
use tracing::{event, Level};
use validator::Validate;

// Defines the cron expression scheduler related env variable names
const SCHEDULER_CRON_EXPRESSION_ENV_VAR: &str = "SCHEDULER_CRON_EXPRESSION";

// Defines the update time window related env variable names
const UPDATE_WINDOW_START_ENV_VAR: &str = "UPDATE_WINDOW_START";
const UPDATE_WINDOW_STOP_ENV_VAR: &str = "UPDATE_WINDOW_STOP";

const SCHEDULER_DEFAULT: &str = "* * * * * * *";

/// The module-wide result type.
type Result<T> = std::result::Result<T, scheduler_error::Error>;

// regex format: HH:MM:SS
lazy_static! {
    pub(crate) static ref VALID_UPDATE_TIME_WINDOW_VARIABLE: Regex =
        Regex::new(r"^(2[0-3]|[01]?[0-9]):([0-5]?[0-9]):([0-5]?[0-9])$").unwrap();
}
#[derive(Validate)]
struct LegacyUpdateWindow {
    #[validate(regex = "VALID_UPDATE_TIME_WINDOW_VARIABLE")]
    start_time: String,
    #[validate(regex = "VALID_UPDATE_TIME_WINDOW_VARIABLE")]
    end_time: String,
}
impl LegacyUpdateWindow {
    fn from_environment() -> Result<Option<Self>> {
        let start_env_var = env::var(UPDATE_WINDOW_START_ENV_VAR);
        let stop_env_var = env::var(UPDATE_WINDOW_STOP_ENV_VAR);

        match (start_env_var, stop_env_var) {
            (Err(_), Ok(_)) | (Ok(_), Err(_)) => {
                scheduler_error::MissingTimeWindowVariableSnafu {
                    message: "missing update time start variable or update time start variable, please provide both of them.".to_string(),
                }.fail()
            },
            (Err(_), Err(_)) => {
                Ok(None)
            },
            (Ok(update_window_start), Ok(update_window_stop)) => {
                Ok(Some(Self::from_string(update_window_start, update_window_stop)))
            }
        }
    }

    fn from_string<S: AsRef<str>>(start_time: S, stop_time: S) -> Self {
        LegacyUpdateWindow {
            start_time: start_time.as_ref().to_string(),
            end_time: stop_time.as_ref().to_string(),
        }
    }

    /// Convert update time window values to cron expression
    /// For overnight schedule, we format it to `{}-23,0-{}` on hour spot.
    /// For example, start time on 18 and stop time on 5. we format it to `18-23,0-5`
    fn cron_expression_converter(&self) -> Result<String> {
        match self.validate() {
            Ok(_) => {
                let start_hour: u8 = self
                    .start_time
                    .split(':')
                    .next()
                    .context(scheduler_error::InvalidTimeWindowSettingsSnafu {})?
                    .parse()
                    .context(scheduler_error::UnableParseToU8Snafu {
                        variable: "start time hour".to_string(),
                    })?;
                let stop_hour: u8 = self
                    .end_time
                    .split(':')
                    .next()
                    .context(scheduler_error::InvalidTimeWindowSettingsSnafu {})?
                    .parse()
                    .context(scheduler_error::UnableParseToU8Snafu {
                        variable: "stop time hour".to_string(),
                    })?;

                let cron_expression_hour = if start_hour <= stop_hour {
                    format!("* * {}-{} * * * *", start_hour, stop_hour)
                } else {
                    format!("* * {}-23,0-{} * * * *", start_hour, stop_hour)
                };

                Ok(cron_expression_hour)
            }
            Err(_) => scheduler_error::InvalidTimeWindowSettingsSnafu {}.fail(),
        }
    }
}

pub struct BrupopCronScheduler {
    scheduler: cron::Schedule,
    schedule_type: ScheduleType,
}

impl BrupopCronScheduler {
    pub fn from_environment() -> Result<Self> {
        let legacy_window = LegacyUpdateWindow::from_environment()?;
        let scheduler_cron_expression = get_cron_schedule_from_env()?;

        let scheduler = match (legacy_window, scheduler_cron_expression) {
            // it's not allowed to set update time window and scheduler at same time, cron expression takes precedent
            (Some(_), Some(cron_schedule)) => {
                event!(
                    Level::WARN,
                    "Both time window and cron expression provided - using cron expression for schedule."
                );
                Ok(cron_schedule)
            }
            // set cron expression scheduler to default "* * * * * * *" if no update time window and
            // scheduler variables are provided.
            (None, None) => Ok(SCHEDULER_DEFAULT.to_string()),
            // convert update time window variable to cron expression format if users only provide
            // update time window variables.
            (Some(update_time_window), None) => Ok(update_time_window.cron_expression_converter()?),
            // set cron expression scheduler to the value that users provide
            (None, Some(cron_schedule)) => Ok(cron_schedule),
        }?;

        Self::from_string(scheduler)
    }

    pub async fn wait_until_next_maintainence_window(&self) -> Result<()> {
        let now = Utc::now();
        let duration_to_next = self.duration_to_next(now)?;
        let std_duration = std_duration(&duration_to_next)?;
        let seconds = std_duration.as_secs();
        let next_schedule_time = now + duration_to_next;
        event!(
            Level::INFO,
            next_schedule_time = ?next_schedule_time,
            sleep = seconds,
            "Sleeping until next scheduled time point."
        );
        tokio::time::sleep(std_duration).await;
        Ok(())
    }

    /// Determine when controller needs discontinue updates.
    /// specific trigger time => never discontinue updates.
    /// maintenance window (time window): discontinue updates when current is outside of a scheduled window.
    pub fn should_discontinue_updates(&self) -> bool {
        self.should_discontinue_updates_impl(Utc::now())
    }

    fn from_string<S: AsRef<str>>(cron_expression: S) -> Result<Self> {
        let scheduler = Schedule::from_str(cron_expression.as_ref()).context(
            scheduler_error::GenerateScheduleFailedSnafu {
                cron: cron_expression.as_ref(),
            },
        )?;
        let schedule_type = determine_schedule_type(&scheduler)?;
        Ok(BrupopCronScheduler {
            scheduler,
            schedule_type,
        })
    }

    fn duration_to_next(&self, now: DateTime<Utc>) -> Result<chrono::Duration> {
        let next_scheduled_time = self
            .scheduler
            .after(&now)
            .next()
            .context(scheduler_error::GetScheduledDatetimeSnafu)?;
        Ok(next_scheduled_time - now)
    }

    fn should_discontinue_updates_impl(&self, now: DateTime<Utc>) -> bool {
        match self.schedule_type {
            ScheduleType::Windowed => {
                if self.scheduler.includes(now) {
                    return false;
                }
            }
            ScheduleType::Oneshot => return false,
        }
        true
    }
}

/// Cron expression can be configured to a time window or a specific trigger time.
/// specific trigger time: 0 0 10 * * Mon */Every Monday at 10AM.
/// maintenance window (time window): * * 10-12 * * MON */Every Monday between 10:00 and 12:00.

/// brupop controller needs to use different logics to deal with specific trigger time or
/// maintenance window (time window)
/// => specific trigger time: trigger brupop update and complete all waitingForUpdate nodes.
/// => maintenance window (time window): trigger brupop update within time window. If current
/// time isn't within the time window, controller shouldn't have any action on it.
#[derive(PartialEq, Debug)]
pub enum ScheduleType {
    Windowed,
    Oneshot,
}

fn determine_schedule_type(schedule: &Schedule) -> Result<ScheduleType> {
    let duration_between_each_schedule_datetime =
        duration_between_next_two_points(schedule, Utc::now())?;
    Ok(
        if duration_between_each_schedule_datetime.num_seconds() == 1 {
            ScheduleType::Windowed
        } else {
            ScheduleType::Oneshot
        },
    )
}

fn duration_between_next_two_points(
    schedule: &Schedule,
    from: DateTime<Utc>,
) -> Result<chrono::Duration> {
    let first_time = schedule
        .after(&from)
        .next()
        .context(scheduler_error::GetScheduledDatetimeSnafu)?;
    let second_time = schedule
        .after(&from)
        .nth(1)
        .context(scheduler_error::GetScheduledDatetimeSnafu)?;
    Ok(second_time - first_time)
}

fn std_duration(d: &chrono::Duration) -> Result<std::time::Duration> {
    d.to_std()
        .context(scheduler_error::ConvertToStdDurationSnafu)
}

fn get_cron_schedule_from_env() -> Result<Option<String>> {
    match env::var(SCHEDULER_CRON_EXPRESSION_ENV_VAR) {
        // SCHEDULER_CRON_EXPRESSION is set
        Ok(scheduler_cron_expression) => Ok(Some(scheduler_cron_expression)),
        // SCHEDULER_CRON_EXPRESSION is not set
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;
    use chrono::{NaiveDate, Utc};

    #[test]
    fn test_duration_between_next_two_points() {
        let test_cases = vec![
            (
                Schedule::from_str("* * * * * * *".as_ref()).unwrap(),
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 1, 1)
                        .unwrap()
                        .and_hms_opt(2, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                chrono::Duration::seconds(1),
            ),
            (
                Schedule::from_str("10 10 10 * * * *".as_ref()).unwrap(),
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 1, 1)
                        .unwrap()
                        .and_hms_opt(2, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                chrono::Duration::hours(24),
            ),
            (
                Schedule::from_str("10 10 10 * * Mon *".as_ref()).unwrap(),
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 1, 1)
                        .unwrap()
                        .and_hms_opt(2, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                chrono::Duration::days(7),
            ),
        ];
        for (schedule, from, result) in test_cases {
            assert_eq!(
                duration_between_next_two_points(&schedule, from).unwrap(),
                result
            )
        }
    }

    #[test]
    fn test_duration_to_next() {
        let test_cases = vec![
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(2, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "* * 4 1 12 * 2099",
                chrono::Duration::hours(2),
            ),
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "* * * 31 12 * 2099",
                chrono::Duration::days(30),
            ),
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "1 * * 1 12 * 2099",
                chrono::Duration::seconds(1),
            ),
        ];

        for (now, cron_expression, result) in test_cases {
            let brupop_cron_scheduler = BrupopCronScheduler::from_string(cron_expression).unwrap();
            assert_eq!(brupop_cron_scheduler.duration_to_next(now).unwrap(), result);
        }
    }

    #[test]
    fn test_should_discontinue_updates_impl() {
        let test_cases = vec![
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(2, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "* * * * * * *",
                false,
            ),
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "10 10 10 * * * *",
                false,
            ),
            (
                DateTime::<Utc>::from_naive_utc_and_offset(
                    NaiveDate::from_ymd_opt(2099, 12, 1)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                    Utc,
                ),
                "* * 10 * * * *",
                true,
            ),
        ];
        for (now, cron_expression, result) in test_cases {
            let brupop_cron_scheduler = BrupopCronScheduler::from_string(cron_expression).unwrap();
            assert_eq!(
                brupop_cron_scheduler.should_discontinue_updates_impl(now),
                result
            );
        }
    }

    #[test]
    fn test_cron_expression_converter() {
        let test_cases = vec![
            ("0:0:0", "5:0:0", "* * 0-5 * * * *"),
            ("21:0:0", "8:30:0", "* * 21-23,0-8 * * * *"),
            ("15:0:0", "3:30:34", "* * 15-23,0-3 * * * *"),
        ];

        for (start_time, end_time, result) in test_cases {
            let update_time_window = LegacyUpdateWindow::from_string(start_time, end_time);
            assert_eq!(
                update_time_window.cron_expression_converter().unwrap(),
                result
            );
        }
    }

    #[test]
    fn test_from_environment() {
        // These would normally be separate unit tests for each case, but since
        // they rely on environment variables as input they are done sequentally
        // here.

        // Legacy update window usage
        env::remove_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR);
        env::set_var(UPDATE_WINDOW_START_ENV_VAR, "09:00:00");
        env::set_var(UPDATE_WINDOW_STOP_ENV_VAR, "21:00:00");

        let result = BrupopCronScheduler::from_environment().unwrap();
        let expected = Schedule::from_str("* * 9-21 * * * *").unwrap();
        assert!(result.scheduler.timeunitspec_eq(&expected));

        // Legacy update window missing start time
        env::remove_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR);
        env::remove_var(UPDATE_WINDOW_START_ENV_VAR);
        env::set_var(UPDATE_WINDOW_STOP_ENV_VAR, "21:00:00");

        let result = LegacyUpdateWindow::from_environment();
        assert!(result.is_err());

        // Legacy update window missing stop time
        env::remove_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR);
        env::set_var(UPDATE_WINDOW_START_ENV_VAR, "09:00:00");
        env::remove_var(UPDATE_WINDOW_STOP_ENV_VAR);

        let result = LegacyUpdateWindow::from_environment();
        assert!(result.is_err());

        // Cron expression
        env::set_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR, "* * 5 * * * *");
        env::remove_var(UPDATE_WINDOW_START_ENV_VAR);
        env::remove_var(UPDATE_WINDOW_STOP_ENV_VAR);

        let result = BrupopCronScheduler::from_environment().unwrap();
        let expected = Schedule::from_str("* * 5 * * * *").unwrap();
        assert!(result.scheduler.timeunitspec_eq(&expected));

        // Cron expression as the default result
        env::remove_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR);
        env::remove_var(UPDATE_WINDOW_START_ENV_VAR);
        env::remove_var(UPDATE_WINDOW_STOP_ENV_VAR);

        let result = BrupopCronScheduler::from_environment().unwrap();
        let expected = Schedule::from_str("* * * * * * *").unwrap();
        assert!(result.scheduler.timeunitspec_eq(&expected));

        // Cron expression and legacy window both provided
        env::set_var(SCHEDULER_CRON_EXPRESSION_ENV_VAR, "* * 5 * * * *");
        env::set_var(UPDATE_WINDOW_START_ENV_VAR, "09:00:00");
        env::set_var(UPDATE_WINDOW_STOP_ENV_VAR, "21:00:00");

        let result = BrupopCronScheduler::from_environment().unwrap();
        let expected = Schedule::from_str("* * 5 * * * *").unwrap();
        assert!(result.scheduler.timeunitspec_eq(&expected));
    }
}

pub mod scheduler_error {
    use snafu::Snafu;
    use std::num::ParseIntError;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub))]
    pub enum Error {
        #[snafu(display("Unable convert to Std duration due to {}", source))]
        ConvertToStdDuration { source: chrono::OutOfRangeError },

        #[snafu(display("Failed to generate corn expression '{}' due to `{}`", cron, source))]
        GenerateScheduleFailed {
            cron: String,
            source: cron::error::Error,
        },

        #[snafu(display("Unable to get cron expression schedule scheduled datetime"))]
        GetScheduledDatetime {},

        #[snafu(display(
            "Unable to get environment variable '{}' due to : '{}'",
            variable,
            source
        ))]
        MissingEnvVariable {
            source: std::env::VarError,
            variable: String,
        },

        #[snafu(display("Failed to find update time window due to '{}'", message))]
        MissingTimeWindowVariable { message: String },

        #[snafu(display(
            "Update time window and scheduler are't allowed to be set simultaneously"
        ))]
        DisallowSetTimeWindowAndScheduler {},

        #[snafu(display(
            "Failed to generate update window settings due to invalid input, please follow HH:MM:SS format."
        ))]
        InvalidTimeWindowSettings {},

        #[snafu(display("Failed to parse {} to u8 due to {}.", variable, source))]
        UnableParseToU8 {
            variable: String,
            source: ParseIntError,
        },
    }
}

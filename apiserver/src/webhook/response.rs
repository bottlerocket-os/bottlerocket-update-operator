use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversionResponse {
    pub kind: String,
    pub api_version: String,
    pub response: Response,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct ConvertResult {
    pub status: Status,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum Status {
    Success,
    Failed,
}

impl Default for ConvertResult {
    fn default() -> Self {
        ConvertResult {
            status: Status::Success,
            message: None,
        }
    }
}

impl ConvertResult {
    pub fn create_fail_result(msg: String) -> Self {
        ConvertResult {
            status: Status::Failed,
            message: Some(msg),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub uid: String,
    pub result: ConvertResult,
    pub converted_objects: Option<Vec<serde_json::Value>>,
}

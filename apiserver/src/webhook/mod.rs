mod request;
mod response;

pub use self::request::{ConversionRequest, Request};
pub use self::response::{ConversionResponse, ConvertResult, Response};

use models::node::v1::BottlerocketShadow as BottleRocketShadowV1;
use models::node::v2::BottlerocketShadow as BottlerocketShadowV2;

use snafu::{ResultExt, Snafu};
use std::convert::TryFrom;
use tracing::instrument;

pub type Result<T> = std::result::Result<T, WebhookConvertError>;

/// Convert k8s ConversionReview object from request to response
/// by applying chained convert methods on its objects.
///
/// Sample request in yaml format:
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// {
///     "apiVersion": "apiextensions.k8s.io/v1",
///     "kind": "ConversionReview",
///     "request": {
///         # Random uid uniquely identifying this conversion call
///         "uid": "5a6adc7e-c74b-43c0-9718-293de1b104cb",
///
///         # The API group and version the objects should be converted to
///         "desiredAPIVersion": "brupop.bottlerocket.aws/v2",
///
///         # The list of objects to convert.
///         # May contain one or more objects, in one or more versions.
///         "objects": [
///             {
///                 "kind": "BottlerocketShadow",
///                 "apiVersion": "brupop.bottlerocket.aws/v1",
///                 "metadata": {
///                     "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
///                     "namespace": "brupop-bottlerocket-aws",
///                     "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266"
///                 },
///                 "spec": {
///                     "state": "Idle",
///                 },
///                 "status": {
///                     "current_state": "Idle",
///                     "target_version": "1.8.0",
///                     "current_version": "1.8.0"
///                 }
///             }
///         ]
///
///     }
/// }
/// ```
/// Sample response in yaml format:
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// {
///     "apiVersion": "apiextensions.k8s.io/v1",
///     "kind": "ConversionReview",
///     "response": {
///         # must match <request.uid>
///         "uid": "5a6adc7e-c74b-43c0-9718-293de1b104cb",
///
///         "result": {
///             "status": "Success"
///         },
///
///         # Objects must match the order of request.objects, and have apiVersion set to <request.desiredAPIVersion>.
///         # kind, metadata.uid, metadata.name, and metadata.namespace fields must not be changed by the webhook.
///         # metadata.labels and metadata.annotations fields may be changed by the webhook.
///         # All other changes to metadata fields by the webhook are ignored.
///         "objects": [
///             {
///                 "kind": "BottlerocketShadow",
///                 "apiVersion": "brupop.bottlerocket.aws/v2",
///                 "metadata": {
///                     "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
///                     "namespace": "brupop-bottlerocket-aws",
///                     "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266"
///                 },
///                 "spec": {
///                     "state": "Idle",
///                 },
///                 "status": {
///                     "current_state": "Idle",
///                     "target_version": "1.8.0",
///                     "current_version": "1.8.0"
///                 }
///             }
///         ]
///
///     }
/// }
/// ```
pub fn convert_request_to_response(req: &ConversionRequest) -> ConversionResponse {
    let request = &req.request;
    let desired_version = request.desired_api_version.clone();

    match convert_objects(desired_version, request.objects.clone()) {
        Ok(new_objects) => {
            let response = Response {
                uid: request.uid.clone(),
                result: ConvertResult::default(),
                converted_objects: Some(new_objects),
            };
            ConversionResponse {
                kind: req.kind.clone(),
                api_version: req.api_version.clone(),
                response,
            }
        }
        Err(e) => {
            let fail_result = ConvertResult::create_fail_result(e.to_string());
            let response = Response {
                uid: request.uid.clone(),
                result: fail_result,
                converted_objects: None,
            };
            ConversionResponse {
                kind: req.kind.clone(),
                api_version: req.api_version.clone(),
                response,
            }
        }
    }
}

#[instrument(err)]
fn convert_objects(
    desired_version: String,
    objects: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    let mut new_objects = Vec::new();
    for old_object in objects.into_iter() {
        let old_brs_object = BRSObject { object: old_object };
        let new_brs_object = old_brs_object.chained_convert_object(desired_version.clone())?;
        new_objects.push(new_brs_object.object);
    }
    Ok(new_objects)
}

/// An abstraction over BottlerocketShadow's json value.
/// Its implementation contains the logic to chain convert BottlerocketShadow
/// to a different version.
///
/// To add a new version convert, first add a method build the logic
/// to convert from previous version like:
#[cfg_attr(doctest, doc = " ````no_test")]
/// ```
/// fn to_v2(source_obj: BRSObject) -> Result<BRSObject> {
///     Self::try_from(BottlerocketShadowV2::from(BottleRocketShadowV1::try_from(
///         source_obj,
///     )?))
/// }
/// ```
///
/// Then update `convert_to_next_version` to map the
/// BottlerocketShadow version to the above method.
///
struct BRSObject {
    pub object: serde_json::Value,
}

impl BRSObject {
    fn get_version(&self) -> Result<String> {
        serde_json::from_value(self.object["apiVersion"].clone())
            .context(SourceVersionNotExistInRequestSnafu)
    }

    fn to_v2(source_obj: BRSObject) -> Result<BRSObject> {
        Self::try_from(BottlerocketShadowV2::from(BottleRocketShadowV1::try_from(
            source_obj,
        )?))
    }

    fn to_v1(source_obj: BRSObject) -> Result<BRSObject> {
        Self::try_from(BottleRocketShadowV1::from(BottlerocketShadowV2::try_from(
            source_obj,
        )?))
    }

    // Since we ware supporting/ship both v1 and v2 versions of the bottlerocketshadow CRD,
    // the CRD conversion webhook needs to also support conversions between the two.
    // Primarily, the kube-api server puts a "watcher" on both versions and will attempt
    // to convert to the one found in it's "Stored Versions".
    // This "pinwheel" converter ensures that we support a seamless transition between either.
    //
    // If we ever have the need to support many more versions,
    // this pinwheel converter should use a single CRD version as the "hub" to convert to
    // and from (preventing the need for a large matrix of supported conversions.
    //
    // For reference:
    // https://book.kubebuilder.io/multiversion-tutorial/conversion-concepts.html
    fn pinwheel_convert(self) -> Result<Self> {
        let version = self.get_version()?;
        match version.as_str() {
            "brupop.bottlerocket.aws/v1" => BRSObject::to_v2(self),
            "brupop.bottlerocket.aws/v2" => BRSObject::to_v1(self),
            _ => InvalidVersionSnafu { version }.fail(),
        }
    }

    #[instrument(skip(self), err)]
    fn chained_convert_object(self, desired_version: String) -> Result<Self> {
        let mut version = self.get_version()?;
        let mut source_object = self;

        // Validates desired version can be accepted into the pinwheel converter
        match desired_version.as_str() {
            "brupop.bottlerocket.aws/v1" => {}
            "brupop.bottlerocket.aws/v2" => {}
            _ => {
                return InvalidDesiredVersionSnafu {
                    version: desired_version,
                }
                .fail()
            }
        }

        // Enter the pinwheel converter
        while version != desired_version {
            match source_object.pinwheel_convert() {
                Ok(val) => source_object = val,
                Err(_) => {
                    return ChainedConvertSnafu {
                        src_version: version,
                        dst_version: desired_version,
                    }
                    .fail()
                }
            }
            version = source_object.get_version()?;
        }

        Ok(source_object)
    }
}

impl TryFrom<BRSObject> for BottleRocketShadowV1 {
    type Error = WebhookConvertError;

    fn try_from(obj: BRSObject) -> Result<Self> {
        serde_json::from_value(obj.object).context(JsonToBottlerocketShadowConvertSnafu {
            version: "v1".to_string(),
        })
    }
}

impl TryFrom<BRSObject> for BottlerocketShadowV2 {
    type Error = WebhookConvertError;

    fn try_from(obj: BRSObject) -> Result<Self> {
        serde_json::from_value(obj.object).context(JsonToBottlerocketShadowConvertSnafu {
            version: "v2".to_string(),
        })
    }
}

impl TryFrom<BottlerocketShadowV2> for BRSObject {
    type Error = WebhookConvertError;

    fn try_from(shadow: BottlerocketShadowV2) -> Result<Self> {
        Ok(BRSObject {
            object: serde_json::to_value(shadow).context(BottlerocketShadowToJsonConvertSnafu {
                version: "v2".to_string(),
            })?,
        })
    }
}

impl TryFrom<BottleRocketShadowV1> for BRSObject {
    type Error = WebhookConvertError;

    fn try_from(shadow: BottleRocketShadowV1) -> Result<Self> {
        Ok(BRSObject {
            object: serde_json::to_value(shadow).context(BottlerocketShadowToJsonConvertSnafu {
                version: "v1".to_string(),
            })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        convert_request_to_response, ConversionRequest, ConversionResponse, ConvertResult, Request,
        Response,
    };
    use serde_json::json;

    #[test]
    fn test_convert_upgrade_request_to_response_succeed() {
        let conversion_req = ConversionRequest {
            kind: "ConversionReview".to_string(),
            api_version: "apiextensions.k8s.io/v1".to_string(),
            request: Request {
                uid: "5a6adc7e-c74b-43c0-9718-293de1b104cb".to_string(),
                desired_api_version: "brupop.bottlerocket.aws/v2".to_string(),
                objects: vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v1",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0"
                    }

                })],
            },
        };

        let expected_response = ConversionResponse {
            kind: conversion_req.kind.clone(),
            api_version: conversion_req.api_version.clone(),
            response: Response {
                uid: conversion_req.request.uid.clone(),
                result: ConvertResult::default(),
                converted_objects: Some(vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v2",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                        "state_transition_timestamp": null,
                        "version": null
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0",
                        "crash_count": 0,
                        "state_transition_failure_timestamp": null,
                    }

                })]),
            },
        };

        let converted_response = convert_request_to_response(&conversion_req);
        assert_eq!(converted_response, expected_response);
    }

    #[test]
    fn test_convert_downgrade_request_to_response_succeed() {
        let conversion_req = ConversionRequest {
            kind: "ConversionReview".to_string(),
            api_version: "apiextensions.k8s.io/v1".to_string(),
            request: Request {
                uid: "5a6adc7e-c74b-43c0-9718-293de1b104cb".to_string(),
                desired_api_version: "brupop.bottlerocket.aws/v1".to_string(),
                objects: vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v2",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                        "state_transition_timestamp": null,
                        "version": null
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0",
                        "crash_count": 0,
                        "state_transition_failure_timestamp": null,
                    }

                })],
            },
        };

        let expected_response = ConversionResponse {
            kind: conversion_req.kind.clone(),
            api_version: conversion_req.api_version.clone(),
            response: Response {
                uid: conversion_req.request.uid.clone(),
                result: ConvertResult::default(),
                converted_objects: Some(vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v1",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                        "state_transition_timestamp": null,
                        "version": null
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0",
                    }

                })]),
            },
        };

        let converted_response = convert_request_to_response(&conversion_req);
        assert_eq!(converted_response, expected_response);
    }

    #[test]
    fn test_convert_request_to_response_failed() {
        let conversion_req = ConversionRequest {
            kind: "ConversionReview".to_string(),
            api_version: "apiextensions.k8s.io/v1".to_string(),
            request: Request {
                uid: "5a6adc7e-c74b-43c0-9718-293de1b104cb".to_string(),
                // desired_version not exist
                desired_api_version: "brupop.bottlerocket.aws/-v2".to_string(),
                objects: vec![json!({
                    "apiVersion": "brupop.bottlerocket.aws/v1",
                    "kind": "BottlerocketShadow",
                    "metadata": {
                        "name": "brs-ip-192-168-22-145.us-west-2.compute.internal",
                        "namespace": "brupop-bottlerocket-aws",
                        "uid": "3153df27-6619-4b6b-bc75-adbf92ef7266",
                        "ownerReferences": [
                            {
                                "apiVersion": "v1",
                                "kind": "Node",
                                "name": "ip-192-168-22-145.us-west-2.compute.internal",
                                "uid": "6b714046-3b20-4a79-aaa9-27cf626a2c12"
                            }
                        ]
                    },
                    "spec": {
                        "state": "Idle",
                    },
                    "status": {
                        "current_state": "Idle",
                        "target_version": "1.8.0",
                        "current_version": "1.8.0"
                    }

                })],
            },
        };

        let expected_response = ConversionResponse {
            kind: conversion_req.kind.clone(),
            api_version: conversion_req.api_version.clone(),
            response: Response {
                uid: conversion_req.request.uid.clone(),
                result: ConvertResult::create_fail_result("Desired version brupop.bottlerocket.aws/-v2 is not a valid BottlerocketShadow version".to_string()),
                converted_objects: None,
            },
        };

        let converted_response = convert_request_to_response(&conversion_req);
        assert_eq!(converted_response, expected_response);
    }
}
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum WebhookConvertError {
    #[snafu(display("Source version does not exist in ConversionRequest: {}", source))]
    SourceVersionNotExistInRequest { source: serde_json::Error },

    #[snafu(display(
        "Failed to convert BottlerocketShadow {} to json object due to:{}",
        version,
        source
    ))]
    BottlerocketShadowToJsonConvertError {
        version: String,
        source: serde_json::error::Error,
    },

    #[snafu(display(
        "Failed to convert json object to BottlerocketShadow {} due to: {}",
        version,
        source
    ))]
    JsonToBottlerocketShadowConvertError {
        version: String,
        source: serde_json::error::Error,
    },

    #[snafu(display(
        "Desired version {} is not a valid BottlerocketShadow version",
        version
    ))]
    InvalidDesiredVersionError { version: String },

    #[snafu(display("Version {} does not exist in converting logic", version))]
    InvalidVersionError { version: String },

    #[snafu(display("Failed to convert from {} to {} version", src_version, dst_version))]
    ChainedConvertError {
        src_version: String,
        dst_version: String,
    },
}

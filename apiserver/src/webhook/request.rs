use super::convert_request_to_response;
use super::response::ConversionResponse;

use serde::{Deserialize, Serialize};
#[derive(Deserialize, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionRequest {
    pub kind: String,
    pub api_version: String,
    pub request: Request,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub uid: String,
    #[serde(rename = "desiredAPIVersion")]
    pub desired_api_version: String,
    pub objects: Vec<serde_json::Value>,
}

impl ConversionRequest {
    /// Wrap over convert_request_to_response so
    /// actix_web::web::Json<ConversionRequest> could call the method
    pub fn convert_resource(&self) -> ConversionResponse {
        convert_request_to_response(self)
    }
}

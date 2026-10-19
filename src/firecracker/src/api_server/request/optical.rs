// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vmm::logger::{IncMetric, METRICS};
use vmm::rpc_interface::VmmAction;
use vmm::vmm_config::console::OpticalDriveConfig;

use super::super::parsed_request::{ParsedRequest, RequestError, checked_id};
use super::{Body, StatusCode};

pub(crate) fn parse_put_optical(
    body: &Body,
    id: Option<&str>,
) -> Result<ParsedRequest, RequestError> {
    METRICS.put_api_requests.optical_count.inc();
    let parse = || {
        let id = checked_id(id.ok_or(RequestError::EmptyID)?)?;
        let config: OpticalDriveConfig = serde_json::from_slice(body.raw())?;
        if id != config.drive_id {
            return Err(RequestError::Generic(
                StatusCode::BadRequest,
                "The id from the path does not match the id from the body!".into(),
            ));
        }
        Ok(ParsedRequest::new_sync(VmmAction::UpdateOpticalDrive(
            config,
        )))
    };
    parse().inspect_err(|_| METRICS.put_api_requests.optical_fails.inc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_server::parsed_request::tests::vmm_action_from_request;

    #[test]
    fn media_update_requires_matching_ids_and_accepts_an_empty_drive() {
        for path in [Some("command.iso"), None] {
            let config = OpticalDriveConfig {
                drive_id: "exec".into(),
                path_on_host: path.map(Into::into),
            };
            let body = Body::new(serde_json::to_string(&config).unwrap());
            assert_eq!(
                vmm_action_from_request(parse_put_optical(&body, Some("exec")).unwrap()),
                VmmAction::UpdateOpticalDrive(config)
            );
            parse_put_optical(&body, Some("other")).unwrap_err();
            parse_put_optical(&body, None).unwrap_err();
            parse_put_optical(&body, Some("invalid-id")).unwrap_err();
        }
        for body in [
            "invalid",
            r#"{"drive_id":"exec","path_on_host":4}"#,
            r#"{"drive_id":"exec","unknown":true}"#,
        ] {
            parse_put_optical(&Body::new(body), Some("exec")).unwrap_err();
        }
    }
}

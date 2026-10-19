// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use vmm::logger::{IncMetric, METRICS};
use vmm::rpc_interface::VmmAction;
use vmm::vmm_config::console::DisplayUpdateConfig;

use super::super::parsed_request::{ParsedRequest, RequestError};
use super::Body;

pub(crate) fn parse_patch_display(body: &Body) -> Result<ParsedRequest, RequestError> {
    METRICS.patch_api_requests.display_count.inc();
    let config = serde_json::from_slice::<DisplayUpdateConfig>(body.raw()).inspect_err(|_| {
        METRICS.patch_api_requests.display_fails.inc();
    })?;
    Ok(ParsedRequest::new_sync(VmmAction::UpdateDisplay(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_server::parsed_request::tests::vmm_action_from_request;

    #[test]
    fn display_request_requires_both_unsigned_dimensions() {
        let config = DisplayUpdateConfig {
            width: 1280,
            height: 720,
        };
        assert_eq!(
            vmm_action_from_request(
                parse_patch_display(&Body::new(r#"{"width":1280,"height":720}"#)).unwrap()
            ),
            VmmAction::UpdateDisplay(config)
        );
        for body in [
            "invalid",
            "{}",
            r#"{"width":1280}"#,
            r#"{"width":-1,"height":720}"#,
            r#"{"width":1280,"height":720,"unknown":true}"#,
        ] {
            parse_patch_display(&Body::new(body)).unwrap_err();
        }
    }
}

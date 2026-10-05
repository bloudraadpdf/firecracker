// Copyright 2026 Bloudraad Ltd
// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use std::process::Command;

    fn rejection(args: &[&str]) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_firecracker"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(153), "{args:?}");
        String::from_utf8(output.stderr).unwrap()
    }

    #[test]
    fn test_api_sock_fd_forbids_api_sock() {
        let stderr = rejection(&["--api-sock-fd", "3", "--api-sock", "/a"]);
        assert!(stderr.contains(r#"ForbiddenArgument("api-sock-fd", "api-sock")"#));
    }

    #[test]
    fn test_api_sock_fd_forbids_no_api() {
        let stderr = rejection(&["--api-sock-fd", "3", "--no-api", "--config-file", "/c"]);
        assert!(stderr.contains(r#"ForbiddenArgument("api-sock-fd", "no-api")"#));
    }

    #[test]
    fn test_api_sock_fd_rejects_standard_stream() {
        let stderr = rejection(&["--api-sock-fd", "2"]);
        assert!(stderr.contains(r#"ApiSockFd(Number("2"))"#));
    }
}

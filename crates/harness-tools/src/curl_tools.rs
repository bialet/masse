//! Local-only HTTP tool backed by the `curl` binary.
//!
//! `curl` is deliberately **not** a general web client: it is confined to the
//! Bialet server that `bialet_server_start` opened. Any other host, and any
//! loopback port that no live server owns, is refused. Redirects are off by
//! default so a 3xx cannot hop off the allowed port.

use async_trait::async_trait;
use serde_json::Value as Json;
use url::Host;

use crate::cli::{truncate_lines, CmdSpec};
use crate::tool::{
    opt_bool, opt_str, opt_str_array, opt_u64, req_str, schema_object, Tool, ToolContext,
    ToolResult,
};

const MAX_OUTPUT_LINES: usize = 400;

pub struct Curl;

/// The host and port a request is allowed to reach, or an error message.
///
/// Only loopback hosts are accepted, and the port must belong to a live
/// background process (the Bialet server under test).
async fn allowed_target(ctx: &ToolContext, url: &str) -> Result<u16, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid url `{url}`: {e}"))?;
    let host_ok = match parsed.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if !host_ok {
        return Err(format!(
            "refused: `{url}` is not on loopback. \
             curl is restricted to the Bialet server started by bialet_server_start \
             (127.0.0.1:<port>)."
        ));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("cannot determine the port of `{url}`"))?;
    if !ctx.processes.is_allowed_port(port).await {
        return Err(format!(
            "refused: no Bialet server is listening on 127.0.0.1:{port}. \
             Start one with bialet_server_start, then curl its port."
        ));
    }
    Ok(port)
}

#[async_trait]
impl Tool for Curl {
    fn name(&self) -> &str {
        "curl"
    }

    fn description(&self) -> &str {
        "Send an HTTP request to the **local** Bialet server started by bialet_server_start and return the body. Only loopback hosts on that server's port are allowed; external URLs and any other local port are refused. Use `data` to send a form/JSON body. Redirects do not follow by default."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                (
                    "url",
                    "string",
                    "URL on the local Bialet server, e.g. http://127.0.0.1:7001/.",
                ),
                (
                    "output",
                    "string",
                    "Optional file path to write the body to.",
                ),
                ("method", "string", "HTTP method. Default: GET (POST when `data` is set)."),
                (
                    "data",
                    "string",
                    "Request body. `a=1&b=2` for a form, or a JSON string with a Content-Type header.",
                ),
                (
                    "header",
                    "string",
                    "Extra request header, e.g. `Content-Type: application/json`. Pass an array to send several.",
                ),
                (
                    "follow",
                    "boolean",
                    "Follow redirects. Default: false (a 3xx cannot leave the allowed port).",
                ),
                ("timeout_secs", "integer", "Override the default timeout."),
            ],
            &["url"],
        )
    }

    async fn call(&self, ctx: &ToolContext, args: Json) -> ToolResult {
        let url = match req_str(&args, "url") {
            Ok(u) => u,
            Err(e) => return ToolResult::err(e),
        };
        let allowed_port = match allowed_target(ctx, &url).await {
            Ok(p) => p,
            Err(e) => return ToolResult::err(e),
        };
        let timeout = ctx.timeout(opt_u64(&args, "timeout_secs"));

        let mut a: Vec<String> = vec!["-sS".into()];
        if opt_bool(&args, "follow", false) {
            // Even when the caller opts in, cap the hops so a redirect cannot
            // be chained to exhaust the timeout.
            a.push("-L".into());
            a.push("--max-redirs".into());
            a.push("5".into());
        }
        a.push("--max-time".into());
        a.push(timeout.as_secs().to_string());
        if let Some(method) = opt_str(&args, "method") {
            a.push("-X".into());
            a.push(method);
        }
        for header in opt_str_array(&args, "header") {
            a.push("-H".into());
            a.push(header);
        }
        if let Some(data) = opt_str(&args, "data") {
            a.push("--data".into());
            a.push(data);
        }
        if let Some(output) = opt_str(&args, "output") {
            let path = match ctx.resolve(&output) {
                Ok(p) => p,
                Err(e) => return ToolResult::err(e),
            };
            a.push("-o".into());
            a.push(path.display().to_string());
        }
        a.push("--".into());
        a.push(url);

        let spec = CmdSpec::new("curl").args(a);
        let out = ctx.run(&spec, timeout).await;
        if out.error.is_some() || out.timed_out || out.exit_code != Some(0) {
            return ToolResult::from_cmd(&out);
        }
        let (stdout, _) = truncate_lines(&out.stdout, MAX_OUTPUT_LINES);
        ToolResult {
            status: crate::tool::ToolStatus::Ok,
            output: if stdout.is_empty() {
                format!("ok (no body on port {allowed_port}: written to file)")
            } else {
                stdout
            },
            exit_code: out.exit_code,
            terminate: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new(std::env::temp_dir())
    }

    #[tokio::test]
    async fn external_hosts_are_refused() {
        let ctx = ctx();
        for url in [
            "http://evil.com/",
            "https://example.com/x",
            "http://8.8.8.8/",
        ] {
            let err = allowed_target(&ctx, url).await.unwrap_err();
            assert!(err.contains("not on loopback"), "{url}: {err}");
        }
    }

    #[tokio::test]
    async fn userinfo_cannot_smuggle_an_external_host() {
        let ctx = ctx();
        let err = allowed_target(&ctx, "http://127.0.0.1@evil.com/")
            .await
            .unwrap_err();
        assert!(err.contains("not on loopback"), "{err}");
    }

    #[tokio::test]
    async fn loopback_without_a_server_is_refused() {
        let ctx = ctx();
        for url in [
            "http://127.0.0.1:7001/",
            "http://localhost:7001/",
            "http://[::1]:7001/",
        ] {
            let err = allowed_target(&ctx, url).await.unwrap_err();
            assert!(err.contains("no Bialet server"), "{url}: {err}");
        }
    }

    #[tokio::test]
    async fn loopback_on_the_started_port_is_allowed() {
        let ctx = ctx();
        // A live process on 7001 makes exactly that port reachable.
        ctx.processes
            .start("srv", "sleep", &["60".into()], &ctx.root, None, Some(7001))
            .await
            .unwrap();
        assert_eq!(
            allowed_target(&ctx, "http://127.0.0.1:7001/")
                .await
                .unwrap(),
            7001
        );
        assert_eq!(
            allowed_target(&ctx, "http://localhost:7001/")
                .await
                .unwrap(),
            7001
        );
        // A different loopback port stays blocked.
        assert!(allowed_target(&ctx, "http://127.0.0.1:7002/")
            .await
            .is_err());
        let _ = ctx.processes.stop("srv").await;
    }

    #[tokio::test]
    async fn a_stopped_server_frees_its_port() {
        let ctx = ctx();
        ctx.processes
            .start("srv", "sleep", &["60".into()], &ctx.root, None, Some(7001))
            .await
            .unwrap();
        ctx.processes.stop("srv").await;
        assert!(allowed_target(&ctx, "http://127.0.0.1:7001/")
            .await
            .is_err());
    }
}

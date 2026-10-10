//! Web download tool backed by the `curl` binary.

use async_trait::async_trait;
use serde_json::Value as Json;

use crate::cli::{truncate_lines, CmdSpec};
use crate::tool::{
    opt_bool, opt_str, opt_str_array, opt_u64, req_str, schema_object, Tool, ToolContext,
    ToolResult,
};

const MAX_OUTPUT_LINES: usize = 400;

pub struct Curl;

#[async_trait]
impl Tool for Curl {
    fn name(&self) -> &str {
        "curl"
    }

    fn description(&self) -> &str {
        "Fetch a URL with the system `curl`. Returns the body, or writes it to a file inside the project when `output` is set. Follows redirects by default. Use `data` to send a request body (form fields like `a=1&b=2`, or JSON with a `Content-Type` header)."
    }

    fn schema(&self) -> Json {
        schema_object(
            &[
                ("url", "string", "URL to fetch (http or https)."),
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
                ("follow", "boolean", "Follow redirects. Default: true."),
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
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return ToolResult::err("url must start with http:// or https://");
        }
        let timeout = ctx.timeout(opt_u64(&args, "timeout_secs"));

        let mut a: Vec<String> = vec!["-sS".into()];
        if opt_bool(&args, "follow", true) {
            a.push("-L".into());
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
                "ok (no body: written to file)".into()
            } else {
                stdout
            },
            exit_code: out.exit_code,
            terminate: None,
        }
    }
}

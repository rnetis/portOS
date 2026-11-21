//! Minimal JSON POST client for webhook delivery (ureq, blocking, run on
//! the blocking pool). Payloads carry an HMAC-SHA256 signature header so
//! receivers can verify authenticity (PRD I-1, §6.3).

/// POST JSON with signature headers; returns the HTTP status.
pub fn post_json(
    url: &str,
    payload: &str,
    signature: &str,
    timeout_secs: u64,
) -> Result<u16, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build();
    match agent
        .post(url)
        .set("content-type", "application/json")
        .set("x-portos-signature", &format!("sha256={signature}"))
        .set("x-portos-event", "delivery")
        .send_string(payload)
    {
        Ok(resp) => Ok(resp.status()),
        Err(ureq::Error::Status(code, _)) => Ok(code), // non-2xx is still a delivery
        Err(e) => Err(format!("webhook post: {e}")),
    }
}

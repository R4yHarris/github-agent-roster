use std::net::Ipv4Addr;

use crate::Error;

pub const LOCAL_REQUEST_TIMEOUT_MS: u32 = 1_200_000;
pub const CLOUD_REQUEST_TIMEOUT_MS: u32 = 120_000;
pub const REMOTE_STREAM_IDLE_TIMEOUT_MS: u32 = 180_000;
pub const WAITING_INTERVAL_MS: u32 = 30_000;

pub fn is_llm_timeout(error: &Error) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if matches!(error.code, Some("ROSTER_LLM_TIMEOUT" | "ROSTER_LLM_STALL")) {
            return true;
        }
        current = error.cause.as_deref();
    }
    false
}

pub fn timeout_hint(
    host: &str,
    timeout_ms: u32,
    local: bool,
    retry_command: Option<&str>,
) -> String {
    let seconds = ryu_js::Buffer::new()
        .format_finite(timeout_ms as f64 / 1000.0)
        .to_owned();
    format!(
        "The LLM request timed out after {seconds}s at host={host}. {}This is an endpoint timeout, not a bad TASK. {}",
        if local {
            "Cold-start: the host may still be warming; Spark/SGLang can take up to 15m. "
        } else {
            ""
        },
        retry_command.map_or_else(
            || "Retry the same request.".to_owned(),
            |command| format!("Retry: {command}")
        )
    )
}

pub fn is_local_llm_host(hostname: &str) -> bool {
    let hostname = hostname.to_lowercase();
    let host = hostname.strip_prefix('[').unwrap_or(&hostname);
    let host = host.strip_suffix(']').unwrap_or(host);
    if host == "localhost" || host == "::1" {
        return true;
    }
    let Ok(address) = host.parse::<Ipv4Addr>() else {
        return false;
    };
    let [first, second, _, _] = address.octets();
    first == 127
        || first == 10
        || first == 192 && second == 168
        || first == 172 && (16..=31).contains(&second)
}

pub fn validate_timeout(value: f64, field: &str) -> Result<u32, Error> {
    if !value.is_finite() || value.fract() != 0.0 || !(1.0..=2_147_483_647.0).contains(&value) {
        return Err(Error::type_error(format!(
            "{field} must be an integer between 1 and 2147483647."
        )));
    }
    Ok(value as u32)
}

pub fn request_timeout(
    hostname: &str,
    request_timeout_ms: Option<f64>,
    timeout_ms: Option<f64>,
) -> Result<u32, Error> {
    if let Some(timeout) = request_timeout_ms {
        return validate_timeout(timeout, "llm.request_timeout_ms");
    }
    if let Some(timeout) = timeout_ms {
        return validate_timeout(timeout, "llm.timeout_ms");
    }
    Ok(if is_local_llm_host(hostname) {
        LOCAL_REQUEST_TIMEOUT_MS
    } else {
        CLOUD_REQUEST_TIMEOUT_MS
    })
}

pub fn stream_idle_timeout(hostname: &str, configured_ms: Option<f64>) -> Result<u32, Error> {
    if let Some(timeout) = configured_ms {
        if timeout == 0.0 {
            return Ok(0);
        }
        return validate_timeout(timeout, "llm.stream_idle_timeout_ms");
    }
    Ok(if is_local_llm_host(hostname) {
        0
    } else {
        REMOTE_STREAM_IDLE_TIMEOUT_MS
    })
}

fn positive_decimal(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|digit| matches!(digit, b'1'..=b'9'))
        && value.bytes().all(|digit| digit.is_ascii_digit())
}

pub fn retry_command_for_task(task: Option<&str>) -> String {
    if let Some(number) = task
        .and_then(|task| task.strip_prefix("issue-"))
        .filter(|number| positive_decimal(number))
    {
        format!("roster run --issue {number}")
    } else {
        "roster run --seat coder --runtime builtin".to_owned()
    }
}

pub fn validate_retry_command(command: Option<&str>) -> Result<Option<&str>, Error> {
    let Some(command) = command else {
        return Ok(None);
    };
    if matches!(
        command,
        "roster run --seat coder --runtime builtin" | "roster doctor --warm"
    ) || command
        .strip_prefix("roster run --issue ")
        .is_some_and(|number| {
            positive_decimal(number.strip_suffix(" --auto-model").unwrap_or(number))
        })
    {
        return Ok(Some(command));
    }
    Err(Error::type_error(
        "LLM retry command must name a supported Roster operation",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ipv4_and_loopback_hosts_are_local_without_dns() {
        for host in [
            "localhost",
            "LOCALHOST",
            "::1",
            "[::1]",
            "10.0.0.1",
            "127.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "172.31.255.255",
        ] {
            assert!(is_local_llm_host(host), "{host}");
        }
        for host in [
            "example.invalid",
            "169.254.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "192.169.0.1",
            "127.01.0.1",
            "127.1",
            "localhost.",
            "0:0:0:0:0:0:0:1",
        ] {
            assert!(!is_local_llm_host(host), "{host}");
        }
    }

    #[test]
    fn timeout_overrides_are_explicit_and_local_idle_is_disabled() -> Result<(), Error> {
        assert_eq!(request_timeout("localhost", None, None)?, 1_200_000);
        assert_eq!(request_timeout("example.invalid", None, None)?, 120_000);
        assert_eq!(request_timeout("localhost", Some(25.0), Some(50.0))?, 25);
        assert_eq!(stream_idle_timeout("localhost", None)?, 0);
        assert_eq!(stream_idle_timeout("example.invalid", None)?, 180_000);
        assert_eq!(stream_idle_timeout("example.invalid", Some(0.0))?, 0);
        for value in [0.0, -1.0, 1.5, f64::NAN, f64::INFINITY, 2_147_483_648.0] {
            assert!(validate_timeout(value, "field").is_err());
        }
        Ok(())
    }

    #[test]
    fn retry_commands_cannot_inject_shell_or_new_operations() -> Result<(), Error> {
        assert_eq!(
            retry_command_for_task(Some("issue-42")),
            "roster run --issue 42"
        );
        assert_eq!(
            validate_retry_command(Some("roster doctor --warm"))?,
            Some("roster doctor --warm")
        );
        for command in [
            "roster run --issue 01",
            "roster run --issue 1;whoami",
            "roster run --issue 1 --publish",
            "roster doctor --warm\n",
            "node worker.mjs",
        ] {
            assert!(validate_retry_command(Some(command)).is_err());
        }
        Ok(())
    }
}

use crate::json::Value;
use crate::text::Text;
use crate::{Error, config, vault};

pub fn resolve_secret(
    name: &Text,
    environment: &Value,
    get: &mut impl FnMut(&str) -> Result<Option<Value>, Error>,
) -> Result<Option<Text>, Error> {
    vault::validate_secret_name(name)?;
    if !matches!(environment, Value::Object(_)) {
        return Err(Error::type_error("A secret environment must be an object."));
    }
    let name = name
        .to_utf8()
        .map_err(|_| Error::from(vault::VaultError::Name))?;
    let selected = config::environment_entry(environment, &name).map_err(|_| {
        Error::type_error("A secret environment name has ambiguous Windows aliases.")
    })?;
    if let Some(value) = selected {
        let value = value
            .as_string()
            .ok_or_else(|| Error::type_error("A secret environment value must be a string."))?;
        if !value.0.is_empty() {
            return Ok(Some(value.clone()));
        }
    }
    let value = get(&name).map_err(|_| {
        Error::new("Error", "Unable to resolve the secret from the file vault.")
            .coded("ROSTER_SECRET_RESOLUTION")
    })?;
    value
        .map(|value| {
            value
                .as_string()
                .filter(|value| !value.0.is_empty())
                .cloned()
                .ok_or_else(|| Error::type_error("The file vault returned an invalid secret."))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_wins_without_touching_a_provider() -> Result<(), Error> {
        let mut calls = 0;
        let resolved = resolve_secret(
            &Text::from("TOKEN"),
            &Value::object(vec![("TOKEN", Value::string("environment-value"))]),
            &mut |_| {
                calls += 1;
                Err(Error::new("Fixture", "must not read"))
            },
        )?;
        assert_eq!(resolved, Some(Text::from("environment-value")));
        assert_eq!(calls, 0);
        Ok(())
    }

    #[test]
    fn unset_empty_and_missing_fallbacks_are_distinct_from_invalid_values() -> Result<(), Error> {
        for environment in [Value::Null, Value::Number(7.0), Value::Array(Vec::new())] {
            let mut calls = 0;
            assert!(
                resolve_secret(&Text::from("TOKEN"), &environment, &mut |_| {
                    calls += 1;
                    Ok(None)
                })
                .is_err_and(|error| error.name == "TypeError")
            );
            assert_eq!(calls, 0);
        }
        for environment in [
            Value::object(Vec::new()),
            Value::object(vec![("TOKEN", Value::string(""))]),
        ] {
            assert_eq!(
                resolve_secret(&Text::from("TOKEN"), &environment, &mut |_| Ok(Some(
                    Value::string("stored-value")
                )))?,
                Some(Text::from("stored-value")),
            );
            assert_eq!(
                resolve_secret(&Text::from("TOKEN"), &environment, &mut |_| Ok(None))?,
                None
            );
            for invalid in [Value::Null, Value::Number(123.0), Value::string("")] {
                let failure = resolve_secret(&Text::from("TOKEN"), &environment, &mut |_| {
                    Ok(Some(invalid.clone()))
                });
                assert!(
                    failure.is_err_and(|error| error.name == "TypeError"
                        && error.message.contains("invalid secret"))
                );
            }
        }
        for value in [Value::Null, Value::Number(123.0), Value::Boolean(false)] {
            let mut calls = 0;
            let failure = resolve_secret(
                &Text::from("TOKEN"),
                &Value::object(vec![("TOKEN", value)]),
                &mut |_| {
                    calls += 1;
                    Ok(None)
                },
            );
            assert!(failure.is_err_and(
                |error| error.name == "TypeError" && error.message.contains("must be a string")
            ));
            assert_eq!(calls, 0);
        }
        assert_eq!(
            resolve_secret(
                &Text::from("TOKEN"),
                &Value::object(vec![("TOKEN", Value::string(" "))]),
                &mut |_| Ok(None)
            )?,
            Some(Text::from(" ")),
        );
        Ok(())
    }

    #[test]
    fn invalid_names_and_provider_failures_are_fixed_and_causeless() {
        for name in ["../TOKEN", "GITHUB_APP_ID", "github_app_key", ""] {
            let mut calls = 0;
            assert!(
                resolve_secret(&Text::from(name), &Value::object(Vec::new()), &mut |_| {
                    calls += 1;
                    Ok(None)
                })
                .is_err()
            );
            assert_eq!(calls, 0);
        }
        let sentinel = "test-only-secret-that-must-not-leak";
        let failure = resolve_secret(
            &Text::from("TOKEN"),
            &Value::object(Vec::new()),
            &mut |_| Err(Error::new("Fixture", sentinel).caused_by(Error::new("Fixture", sentinel))),
        );
        assert!(failure.is_err_and(|error| {
            error.code == Some("ROSTER_SECRET_RESOLUTION")
                && error.cause.is_none()
                && !format!("{error:?}").contains(sentinel)
        }));
    }
}

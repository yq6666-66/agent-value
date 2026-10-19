//! Arms: the comparison unit `i = (m, r)` (v22 §A).
//!
//! `m` is the primary model the user picked for a task, normalised to
//! "family + version". The route `r` is the provider kind (official,
//! relay, subscription), the endpoint host, the wire protocol, and the
//! agent or client. Any difference in route fields makes a different route,
//! so the same model on two routes is two arms. API keys are not part of a
//! route: switching keys on the same route is not a new arm, and no key value
//! is ever held here.

use std::fmt;

use crate::error::{finite, CoreError, Result};

/// §A: a task whose auxiliary-model token share exceeds 20% is tagged "mixed"
/// (still scored, but filterable in the details).
pub const AUX_MODEL_MIXED_SHARE: f64 = 0.20;

/// Normalised primary model identifier ("family + version").
///
/// Alias resolution (mapping provider-specific names onto one id) is a later
/// step; for now the id is trimmed and lower-cased.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(String);

impl ModelId {
    pub fn new(id: &str) -> Result<Self> {
        let s = id.trim().to_lowercase();
        if s.is_empty() {
            return Err(CoreError::EmptyField { field: "model id" });
        }
        if s.chars().any(char::is_whitespace) {
            return Err(CoreError::InvalidField {
                field: "model id",
                reason: "must not contain whitespace",
            });
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who serves the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProviderKind {
    /// The model vendor's own API ("official direct").
    Official,
    /// A third-party relay or proxy.
    Relay,
    /// A flat-fee subscription plan.
    Subscription,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Relay => "relay",
            Self::Subscription => "subscription",
        }
    }
}

/// Wire protocol of the route.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Protocol {
    AnthropicMessages,
    OpenAiChatCompletions,
    OpenAiResponses,
    /// Any other protocol, by a non-empty lower-case name.
    Other(String),
}

impl Protocol {
    /// Build [`Protocol::Other`] with validation.
    pub fn other(name: &str) -> Result<Self> {
        let s = name.trim().to_lowercase();
        if s.is_empty() {
            return Err(CoreError::EmptyField { field: "protocol" });
        }
        Ok(Self::Other(s))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenAiChatCompletions => "openai-chat-completions",
            Self::OpenAiResponses => "openai-responses",
            Self::Other(s) => s,
        }
    }
}

/// A route `r = (provider kind, host, protocol, client)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Route {
    kind: ProviderKind,
    host: String,
    protocol: Protocol,
    client: String,
}

impl Route {
    /// Validates and normalises a route.
    ///
    /// `host` must be a bare host name (optionally with `:port`): no scheme,
    /// path, query or user-info, so credentials can never ride along in it.
    /// Host and client are trimmed and lower-cased; a trailing dot on the host
    /// is dropped.
    pub fn new(kind: ProviderKind, host: &str, protocol: Protocol, client: &str) -> Result<Self> {
        let host = normalize_host(host)?;
        let client = client.trim().to_lowercase();
        if client.is_empty() {
            return Err(CoreError::EmptyField { field: "client" });
        }
        Ok(Self {
            kind,
            host,
            protocol,
            client,
        })
    }

    pub fn kind(&self) -> ProviderKind {
        self.kind
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn protocol(&self) -> &Protocol {
        &self.protocol
    }

    pub fn client(&self) -> &str {
        &self.client
    }

    /// "Official direct" route (§A, §13). Whether a host really belongs to the
    /// vendor's official host table is checked by a later step; here the
    /// declared provider kind decides.
    pub fn is_official_direct(&self) -> bool {
        self.kind == ProviderKind::Official
    }
}

fn normalize_host(host: &str) -> Result<String> {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    if h.is_empty() {
        return Err(CoreError::EmptyField { field: "host" });
    }
    if h.contains("://") {
        return Err(CoreError::InvalidField {
            field: "host",
            reason: "must be a bare host name without a scheme",
        });
    }
    if h.contains('@') {
        return Err(CoreError::InvalidField {
            field: "host",
            reason: "must not contain user info",
        });
    }
    if h.contains(['/', '?', '#']) || h.chars().any(char::is_whitespace) {
        return Err(CoreError::InvalidField {
            field: "host",
            reason: "must not contain a path, query, fragment or whitespace",
        });
    }
    Ok(h)
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}/{}/{}",
            self.kind.as_str(),
            self.host,
            self.protocol.as_str(),
            self.client
        )
    }
}

/// An arm `i = (model, route)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Arm {
    model: ModelId,
    route: Route,
}

impl Arm {
    pub fn new(model: ModelId, route: Route) -> Self {
        Self { model, route }
    }

    pub fn model(&self) -> &ModelId {
        &self.model
    }

    pub fn route(&self) -> &Route {
        &self.route
    }
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} via {}", self.model.as_str(), self.route)
    }
}

/// §A "mixed" tag: auxiliary-model tokens are more than 20% of the task's
/// tokens. `total_tokens == 0` is not mixed.
pub fn is_mixed_task(aux_tokens: u64, total_tokens: u64) -> Result<bool> {
    if aux_tokens > total_tokens {
        return Err(CoreError::OutOfRange {
            what: "auxiliary tokens",
            value: aux_tokens as f64,
            min: 0.0,
            max: total_tokens as f64,
        });
    }
    if total_tokens == 0 {
        return Ok(false);
    }
    let share = finite("aux token share", aux_tokens as f64 / total_tokens as f64)?;
    Ok(share > AUX_MODEL_MIXED_SHARE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kind: ProviderKind, host: &str) -> Route {
        Route::new(kind, host, Protocol::AnthropicMessages, "example-cli").unwrap()
    }

    #[test]
    fn same_model_two_routes_are_two_arms() {
        let m = ModelId::new("example-model-1").unwrap();
        let a = Arm::new(
            m.clone(),
            route(ProviderKind::Official, "api.vendor.example"),
        );
        let b = Arm::new(m, route(ProviderKind::Relay, "relay.example"));
        assert_ne!(a, b);
    }

    #[test]
    fn any_route_field_difference_is_a_new_route() {
        let base = route(ProviderKind::Official, "api.vendor.example");
        let other_kind = route(ProviderKind::Subscription, "api.vendor.example");
        let other_proto = Route::new(
            ProviderKind::Official,
            "api.vendor.example",
            Protocol::OpenAiChatCompletions,
            "example-cli",
        )
        .unwrap();
        let other_client = Route::new(
            ProviderKind::Official,
            "api.vendor.example",
            Protocol::AnthropicMessages,
            "other-cli",
        )
        .unwrap();
        assert_ne!(base, other_kind);
        assert_ne!(base, other_proto);
        assert_ne!(base, other_client);
    }

    #[test]
    fn normalisation_makes_equal_routes() {
        let a = Route::new(
            ProviderKind::Relay,
            " Relay.Example. ",
            Protocol::other("Custom-Proto").unwrap(),
            "CLI",
        )
        .unwrap();
        let b = Route::new(
            ProviderKind::Relay,
            "relay.example",
            Protocol::other("custom-proto").unwrap(),
            "cli",
        )
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "relay:relay.example/custom-proto/cli");
    }

    #[test]
    fn bad_route_fields_are_errors() {
        let p = || Protocol::AnthropicMessages;
        let k = ProviderKind::Relay;
        assert!(matches!(
            Route::new(k, "", p(), "c"),
            Err(CoreError::EmptyField { .. })
        ));
        assert!(matches!(
            Route::new(k, "h", p(), " "),
            Err(CoreError::EmptyField { .. })
        ));
        for bad in [
            "https://h.example",
            "user:pw@h.example",
            "h.example/v1",
            "h ex",
            "h?x=1",
        ] {
            assert!(
                matches!(
                    Route::new(k, bad, p(), "c"),
                    Err(CoreError::InvalidField { .. })
                ),
                "{bad}"
            );
        }
        assert!(Route::new(k, "h.example:8443", p(), "c").is_ok());
        assert!(Protocol::other(" ").is_err());
        assert!(ModelId::new("").is_err());
        assert!(ModelId::new("a b").is_err());
    }

    #[test]
    fn official_direct() {
        assert!(route(ProviderKind::Official, "api.vendor.example").is_official_direct());
        assert!(!route(ProviderKind::Relay, "relay.example").is_official_direct());
        assert!(!route(ProviderKind::Subscription, "api.vendor.example").is_official_direct());
    }

    #[test]
    fn mixed_task_tag() {
        assert_eq!(is_mixed_task(20, 100), Ok(false));
        assert_eq!(is_mixed_task(21, 100), Ok(true));
        assert_eq!(is_mixed_task(0, 0), Ok(false));
        assert!(is_mixed_task(101, 100).is_err());
    }
}

//! Token accounting and estimated cost.
//!
//! Providers report how many tokens a completion consumed in their `usage`
//! block. The agent sums that per turn and for the session, and the front ends
//! show it. Cost is an estimate: it multiplies the token counts by a price per
//! million tokens, taken from a small built-in table and overridable from the
//! command line, so an unknown model still reports tokens with no price.

/// Tokens consumed by one or more completions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    /// Add another usage into this one, field by field.
    pub fn add(&mut self, other: TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }

    /// True when nothing has been counted (e.g. a provider that sends no usage).
    pub fn is_empty(&self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0 && self.total_tokens == 0
    }

    /// `total_tokens`, falling back to the sum when a provider omits it.
    pub fn normalized_total(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.prompt_tokens + self.completion_tokens
        }
    }
}

/// Price in USD per one million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl ModelPricing {
    /// Estimated cost of `usage`, in USD.
    pub fn cost(&self, usage: TokenUsage) -> f64 {
        usage.prompt_tokens as f64 / 1_000_000.0 * self.input_per_million
            + usage.completion_tokens as f64 / 1_000_000.0 * self.output_per_million
    }
}

/// Built-in prices, matched case-insensitively against a substring of the model
/// id. More specific entries come first so `gpt-4o-mini` wins over `gpt-4o`.
const PRICES: [(&str, ModelPricing); 15] = [
    (
        "deepseek-reasoner",
        ModelPricing {
            input_per_million: 0.55,
            output_per_million: 2.19,
        },
    ),
    (
        "deepseek-chat",
        ModelPricing {
            input_per_million: 0.27,
            output_per_million: 1.10,
        },
    ),
    (
        "deepseek",
        ModelPricing {
            input_per_million: 0.27,
            output_per_million: 1.10,
        },
    ),
    (
        "gpt-4o-mini",
        ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
        },
    ),
    (
        "gpt-4o",
        ModelPricing {
            input_per_million: 2.50,
            output_per_million: 10.00,
        },
    ),
    (
        "gpt-4.1-mini",
        ModelPricing {
            input_per_million: 0.40,
            output_per_million: 1.60,
        },
    ),
    (
        "gpt-4.1",
        ModelPricing {
            input_per_million: 2.00,
            output_per_million: 8.00,
        },
    ),
    (
        "o4-mini",
        ModelPricing {
            input_per_million: 1.10,
            output_per_million: 4.40,
        },
    ),
    (
        "o3-mini",
        ModelPricing {
            input_per_million: 1.10,
            output_per_million: 4.40,
        },
    ),
    (
        "claude-opus",
        ModelPricing {
            input_per_million: 15.00,
            output_per_million: 75.00,
        },
    ),
    (
        "claude-sonnet",
        ModelPricing {
            input_per_million: 3.00,
            output_per_million: 15.00,
        },
    ),
    (
        "claude-haiku",
        ModelPricing {
            input_per_million: 0.80,
            output_per_million: 4.00,
        },
    ),
    (
        "gemini-2.0-flash",
        ModelPricing {
            input_per_million: 0.10,
            output_per_million: 0.40,
        },
    ),
    (
        "gemini-1.5-pro",
        ModelPricing {
            input_per_million: 1.25,
            output_per_million: 5.00,
        },
    ),
    (
        "gemini-1.5-flash",
        ModelPricing {
            input_per_million: 0.075,
            output_per_million: 0.30,
        },
    ),
];

/// Look up the built-in price for a model id. `None` when nothing matches, so
/// the caller can still report tokens without inventing a price.
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    let lower = model.to_lowercase();
    PRICES
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, price)| *price)
}

/// Built-in context windows (tokens), matched the same way as [`PRICES`].
const CONTEXT_WINDOWS: [(&str, u64); 13] = [
    ("deepseek", 128_000),
    ("gpt-4o", 128_000),
    ("gpt-4.1", 1_048_576),
    ("o4-mini", 200_000),
    ("o3-mini", 200_000),
    ("claude-opus", 200_000),
    ("claude-sonnet", 200_000),
    ("claude-haiku", 200_000),
    ("gemini-2.0-flash", 1_048_576),
    ("gemini-1.5-pro", 2_097_152),
    ("gemini-1.5-flash", 1_048_576),
    ("llama", 131_072),
    ("qwen", 131_072),
];

/// Look up the built-in context window for a model id, in tokens. `None` when
/// nothing matches, so the front ends can omit the percentage instead of
/// showing a made-up one.
pub fn context_window_for(model: &str) -> Option<u64> {
    let lower = model.to_lowercase();
    CONTEXT_WINDOWS
        .iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, window)| *window)
}

/// One turn's and the session's usage, plus how full the context window is, as
/// emitted to the front ends.
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageReport {
    pub turn: TokenUsage,
    pub total: TokenUsage,
    pub turn_cost_usd: Option<f64>,
    pub total_cost_usd: Option<f64>,
    /// Prompt tokens of the most recent completion: the size of the context
    /// that was just sent, i.e. how full the window is right now.
    pub context_tokens: u64,
    /// The model's context window, when known.
    pub context_window: Option<u64>,
}

impl UsageReport {
    /// Percentage of the context window in use, when the window is known.
    pub fn context_percent(&self) -> Option<f64> {
        match self.context_window {
            Some(w) if w > 0 => Some(self.context_tokens as f64 / w as f64 * 100.0),
            _ => None,
        }
    }
}

/// Compact token count: `812`, `12.3k`, `1.5M`.
pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// USD amount with four decimals; tiny non-zero amounts never render as `$0`.
pub fn format_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.0001 {
        return "<$0.0001".to_string();
    }
    format!("${usd:.4}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_and_normalizes() {
        let mut u = TokenUsage::default();
        assert!(u.is_empty());
        u.add(TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 0,
        });
        assert!(!u.is_empty());
        assert_eq!(u.normalized_total(), 120, "sum when total is omitted");
        u.add(TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 2,
            total_tokens: 3,
        });
        assert_eq!(u.prompt_tokens, 101);
        assert_eq!(u.total_tokens, 3);
        assert_eq!(u.normalized_total(), 3, "explicit total wins");
    }

    #[test]
    fn pricing_matches_specific_before_general() {
        assert_eq!(pricing_for("gpt-4o-mini").unwrap().input_per_million, 0.15);
        assert_eq!(pricing_for("GPT-4O").unwrap().output_per_million, 10.00);
        let cost = pricing_for("deepseek-chat").unwrap().cost(TokenUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 1_000_000,
            total_tokens: 2_000_000,
        });
        assert!((cost - 1.37).abs() < 1e-9, "0.27 + 1.10 = 1.37, got {cost}");
    }

    #[test]
    fn unknown_model_has_no_price_but_tokens_are_kept() {
        assert!(pricing_for("some-local-llama").is_none());
        assert_eq!(format_tokens(812), "812");
        assert_eq!(format_tokens(12_345), "12.3k");
        assert_eq!(format_tokens(1_500_000), "1.5M");
        assert_eq!(format_cost(0.0), "$0.0000");
        assert_eq!(format_cost(0.0042), "$0.0042");
        assert_eq!(format_cost(0.00001), "<$0.0001");
    }

    #[test]
    fn context_window_and_percentage() {
        assert_eq!(context_window_for("deepseek-chat"), Some(128_000));
        assert_eq!(context_window_for("claude-sonnet-4"), Some(200_000));
        assert_eq!(context_window_for("unknown"), None);

        let report = UsageReport {
            context_tokens: 64_000,
            context_window: Some(128_000),
            ..Default::default()
        };
        assert_eq!(report.context_percent(), Some(50.0));

        let unknown = UsageReport {
            context_tokens: 64_000,
            context_window: None,
            ..Default::default()
        };
        assert_eq!(unknown.context_percent(), None, "no window, no percentage");
    }
}

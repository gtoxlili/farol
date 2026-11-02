//! The board's two languages. The scene's static texts are in
//! [`crate::scene::TEXTS`]; the logic picks its own strings with
//! [`Lang::pick`] and [`tr!`](crate::tr).
//!
//! The language follows the Paseo app (the plugin sends it in each snapshot)
//! and is kept with the board's settings, so the pages before Paseo connects
//! use the last one. A fresh board starts in Chinese, the scene's default; the
//! welcome page switches it.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

impl Lang {
    /// The text for this language.
    pub fn pick<'a>(self, zh: &'a str, en: &'a str) -> &'a str {
        match self {
            Self::Zh => zh,
            Self::En => en,
        }
    }

    /// From the plugin's `lang`: "zh", "en"; `None` for anything else.
    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "zh" => Some(Self::Zh),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Zh => Self::En,
            Self::En => Self::Zh,
        }
    }
}

/// `format!` in the board's language:
/// `tr!(lang, "已重试 {} 次", "Tried {} times", n)`.
#[macro_export]
macro_rules! tr {
    ($lang:expr, $zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $lang {
            $crate::lang::Lang::Zh => alloc::format!($zh $(, $arg)*),
            $crate::lang::Lang::En => alloc::format!($en $(, $arg)*),
        }
    };
}

/// A `text` column with a fixed set of values, mirrored as an enum with `as_str` and `parse`.
/// Values are `tt`, not `literal`: a `literal` fragment reaches derive macros wrapped, so utoipa would miss
/// the `serde(rename)` and document the variant names instead of the values.
///
/// The derives resolve `serde` and `utoipa` in the calling crate (their derive output names those crates
/// anyway), so a crate using the macro depends on both.
#[macro_export]
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $text:tt),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
        pub enum $name {
            $($(#[$variant_meta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<$name> {
                match value {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                $name::parse(&value).ok_or_else(|| format!("unknown {} {value:?}", stringify!($name)))
            }
        }
    };
}

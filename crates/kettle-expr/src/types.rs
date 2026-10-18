//! Kettle's value types and their Trino equivalents.

use serde::{Deserialize, Serialize};

/// A Kettle value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KType {
    String,
    /// Kettle `Number`: a Java double.
    Number,
    /// Kettle `BigNumber`: a Java BigDecimal.
    BigNumber,
    /// Kettle `Integer`: a Java long.
    Integer,
    /// Kettle `Date`: a java.util.Date, millisecond precision.
    Date,
    Timestamp,
    Boolean,
    Binary,
    /// Not known statically (a `None` type, or an input the IR cannot see).
    #[default]
    Unknown,
}

impl KType {
    /// Kettle's type name (`<type>` text) to a type; unknown names are [`KType::Unknown`].
    pub fn parse(name: &str) -> KType {
        match name.trim() {
            "String" | "Internet Address" => KType::String,
            "Number" => KType::Number,
            "BigNumber" => KType::BigNumber,
            "Integer" => KType::Integer,
            "Date" => KType::Date,
            "Timestamp" => KType::Timestamp,
            "Boolean" => KType::Boolean,
            "Binary" | "Serializable" => KType::Binary,
            _ => KType::Unknown,
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, KType::Number | KType::BigNumber | KType::Integer)
    }

    pub fn is_temporal(self) -> bool {
        matches!(self, KType::Date | KType::Timestamp)
    }

    /// The Trino type. `length`/`precision` are Kettle's (-1 when unset).
    pub fn trino(self, length: i32, precision: i32) -> String {
        match self {
            KType::String | KType::Unknown => "varchar".into(),
            KType::Number => "double".into(),
            KType::BigNumber => {
                if length > 0 && length <= 38 {
                    let scale = precision.clamp(0, length);
                    format!("decimal({length}, {scale})")
                } else {
                    "decimal(38, 10)".into()
                }
            }
            KType::Integer => "bigint".into(),
            KType::Date => "timestamp(3)".into(),
            KType::Timestamp => "timestamp(6)".into(),
            KType::Boolean => "boolean".into(),
            KType::Binary => "varbinary".into(),
        }
    }

    /// The type of `a <op> b` for an arithmetic operator: Kettle widens Integer → BigNumber →
    /// Number.
    pub fn widen(a: KType, b: KType) -> KType {
        use KType::*;
        match (a, b) {
            (Number, _) | (_, Number) => Number,
            (BigNumber, _) | (_, BigNumber) => BigNumber,
            (Integer, Integer) => Integer,
            (x, Unknown) | (Unknown, x) => x,
            (x, _) => x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_kettle_types_to_trino() {
        assert_eq!(KType::parse("BigNumber").trino(12, 2), "decimal(12, 2)");
        assert_eq!(KType::parse("BigNumber").trino(-1, -1), "decimal(38, 10)");
        assert_eq!(KType::parse("Date").trino(-1, -1), "timestamp(3)");
        assert_eq!(KType::parse("None"), KType::Unknown);
        assert_eq!(
            KType::widen(KType::Integer, KType::BigNumber),
            KType::BigNumber
        );
    }
}

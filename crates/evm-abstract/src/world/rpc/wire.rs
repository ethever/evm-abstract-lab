//! Strict acquisition checks supplement Alloy's permissive JSON-RPC codec.
//! This module does not model requests, replies or Ethereum result data: Alloy
//! owns those. It enforces the existing response identity and canonical-format
//! contract before the SDK deserializes the response and typed payload.
#[cfg(test)]
mod tests;
use super::{ResponseReason, RpcContext, RpcError, invalid};
use serde::{
    Deserializer,
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::BTreeSet, fmt};

pub(super) fn json_error(context: &RpcContext, source: serde_json::Error) -> RpcError {
    if source.is_data() {
        RpcError::Response {
            context: Box::new(context.clone()),
            reason: ResponseReason::Schema,
            source: Some(source),
        }
    } else {
        RpcError::Json {
            context: Box::new(context.clone()),
            source,
        }
    }
}

pub(super) fn validate(bytes: &[u8], id: u64, context: &RpcContext) -> Result<(), RpcError> {
    struct Envelope(u64);
    impl<'de> Visitor<'de> for Envelope {
        type Value = Result<(Option<Box<RawValue>>, bool), ResponseReason>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a JSON-RPC response object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let (mut version, mut observed, mut result, mut error, mut null_error) =
                (None, None, None, false, false);
            let mut seen = BTreeSet::new();
            while let Some(key) = map.next_key::<String>()? {
                if ["jsonrpc", "id", "result", "error"].contains(&key.as_str())
                    && !seen.insert(key.clone())
                {
                    return Err(serde::de::Error::custom(format!("duplicate field {key}")));
                }
                match key.as_str() {
                    "jsonrpc" => version = map.next_value::<Option<String>>()?,
                    "id" => observed = map.next_value::<Option<u64>>()?,
                    "result" => result = Some(map.next_value::<Box<RawValue>>()?),
                    "error" => {
                        let raw = map.next_value::<Box<RawValue>>()?;
                        null_error = raw.get() == "null";
                        if !null_error {
                            // Alloy defaults a missing error message to an
                            // empty string; acquisition requires its presence.
                            ErrorPolicy
                                .deserialize(&mut serde_json::Deserializer::from_str(raw.get()))
                                .map_err(serde::de::Error::custom)?;
                        }
                        error = true;
                    }
                    _ => {
                        let _ = map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
            }
            let violation = if version.as_deref() != Some("2.0") {
                Some(ResponseReason::Version)
            } else if observed != Some(self.0) {
                Some(ResponseReason::Id {
                    expected: self.0,
                    observed,
                })
            } else if result.is_some() && error {
                Some(ResponseReason::ResultAndError)
            } else if null_error {
                Some(ResponseReason::NullError)
            } else {
                None
            };
            Ok(match violation {
                Some(reason) => Err(reason),
                None => Ok((result, error)),
            })
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let (raw, has_error) = decoder
        .deserialize_map(Envelope(id))
        .map_err(|source| json_error(context, source))?
        .map_err(|reason| invalid(context, reason))?;
    decoder
        .end()
        .map_err(|source| json_error(context, source))?;
    if let Some(raw) = raw {
        if raw.get() != "null" {
            let mut decoder = serde_json::Deserializer::from_str(raw.get());
            Payload(context.method)
                .deserialize(&mut decoder)
                .map_err(|source| json_error(context, source))?;
        }
    } else if !has_error {
        // Error responses are decoded by Alloy. A reply with neither member
        // retains the acquisition model's distinct missing-result error.
        return Err(RpcError::MissingResult {
            context: Box::new(context.clone()),
        });
    }
    Ok(())
}
struct ErrorPolicy;
impl<'de> DeserializeSeed<'de> for ErrorPolicy {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        decoder.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for ErrorPolicy {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an RPC error object with a message")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let mut found = false;
        while let Some(key) = map.next_key::<String>()? {
            found |= key == "message";
            let _ = map.next_value::<serde::de::IgnoredAny>()?;
        }
        if !found {
            return Err(serde::de::Error::custom("missing field message"));
        }
        Ok(())
    }
}
struct Quantity;
impl<'de> DeserializeSeed<'de> for Quantity {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        let text = <String as serde::Deserialize>::deserialize(decoder)?;
        let digits = text
            .strip_prefix("0x")
            .filter(|s| !s.is_empty() && s.len() <= 64)
            .ok_or_else(|| {
                serde::de::Error::custom("quantity requires 0x and 1..=64 hex digits")
            })?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit())
            || (digits.len() > 1 && digits.starts_with('0'))
        {
            return Err(serde::de::Error::custom(
                "noncanonical hexadecimal quantity",
            ));
        }
        Ok(())
    }
}
struct Quantities;
impl<'de> DeserializeSeed<'de> for Quantities {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        struct Items;
        impl<'de> Visitor<'de> for Items {
            type Value = ();
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("canonical quantities")
            }
            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<(), S::Error> {
                while seq.next_element_seed(Quantity)?.is_some() {}
                Ok(())
            }
        }
        decoder.deserialize_seq(Items)
    }
}
// Alloy accepts byte strings without a prefix. Check only the acquisition
// representation policy here; the SDK still decodes and owns the bytes.
struct DataPolicy(Option<usize>);
impl<'de> DeserializeSeed<'de> for DataPolicy {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        let text = <String as serde::Deserialize>::deserialize(decoder)?;
        let digits = text
            .strip_prefix("0x")
            .ok_or_else(|| serde::de::Error::custom("byte data requires 0x prefix"))?;
        if !digits.len().is_multiple_of(2)
            || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.0.is_some_and(|size| digits.len() != size * 2)
        {
            return Err(serde::de::Error::custom("invalid hexadecimal byte data"));
        }
        Ok(())
    }
}
struct Payload(&'static str);
impl<'de> DeserializeSeed<'de> for Payload {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        match self.0 {
            "eth_chainId" | "eth_getBalance" | "eth_getTransactionCount" => {
                Quantity.deserialize(decoder)
            }
            "eth_getCode" | "eth_getStorageAt" => DataPolicy(None).deserialize(decoder),
            "eth_getBlockByNumber" | "eth_getBlockByHash" | "eth_feeHistory" => {
                decoder.deserialize_map(self)
            }
            _ => {
                let _ = <serde::de::IgnoredAny as serde::Deserialize>::deserialize(decoder)?;
                Ok(())
            }
        }
    }
}
impl<'de> Visitor<'de> for Payload {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("typed Ethereum response data")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        let quantities = [
            "number",
            "timestamp",
            "gasLimit",
            "baseFeePerGas",
            "excessBlobGas",
            "blobGasUsed",
            "oldestBlock",
        ];
        let checked: &[&str] = if self.0 == "eth_feeHistory" {
            &["oldestBlock", "baseFeePerBlobGas"]
        } else {
            &[
                "hash",
                "parentHash",
                "number",
                "timestamp",
                "miner",
                "mixHash",
                "gasLimit",
                "baseFeePerGas",
                "excessBlobGas",
                "blobGasUsed",
            ]
        };
        // Some SDK fields accept aliases or defaults. Acquisition retains the
        // canonical required names from its fixed-snapshot wire contract.
        let required: &[&str] = if self.0 == "eth_feeHistory" {
            &["oldestBlock", "baseFeePerBlobGas"]
        } else {
            &[
                "hash",
                "parentHash",
                "number",
                "timestamp",
                "miner",
                "mixHash",
                "gasLimit",
            ]
        };
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if checked.contains(&key.as_str()) && !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate field {key}")));
            }
            if !checked.contains(&key.as_str()) {
                let _ = map.next_value::<serde::de::IgnoredAny>()?;
            } else if quantities.contains(&key.as_str()) {
                let raw = map.next_value::<Box<RawValue>>()?;
                if raw.get() != "null" {
                    Quantity
                        .deserialize(&mut serde_json::Deserializer::from_str(raw.get()))
                        .map_err(serde::de::Error::custom)?;
                } else if required.contains(&key.as_str()) {
                    return Err(serde::de::Error::custom(format!(
                        "null required field {key}"
                    )));
                }
            } else if key == "baseFeePerBlobGas" {
                map.next_value_seed(Quantities)?;
            } else if ["hash", "parentHash", "mixHash"].contains(&key.as_str()) {
                map.next_value_seed(DataPolicy(Some(32)))?;
            } else if key == "miner" {
                map.next_value_seed(DataPolicy(Some(20)))?;
            } else {
                let _ = map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        for field in required {
            if !seen.contains(*field) {
                return Err(serde::de::Error::custom(format!("missing field {field}")));
            }
        }
        Ok(())
    }
}

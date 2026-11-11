use super::{
    LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedRevertData, LiftedRevertDataKind,
};
use crate::LiftedRenderError as E;
use crate::{EntityId, U256};

/// Derived view, never a serialized claim independent of the underlying data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiftedRevertFormat {
    /// Exactly zero recovered bytes.
    Empty,
    /// Complete canonical UTF-8 Error(string) bytes.
    ErrorString(String),
    /// Known Panic selector and one complete numeric word.
    Panic(LiftedDataWord),
    /// Lossless selector and complete source words, without guessed ABI types.
    SelectorWords {
        /// Exact leading four bytes.
        selector: [u8; 4],
        /// Ordered complete words from the recovered source segments.
        words: Vec<LiftedDataWord>,
    },
    /// A complete slice from a tracked returndata epoch.
    Forwarded {
        /// Producing source buffer.
        source: LiftedByteSource,
        /// Source slice offset.
        offset: LiftedDataWord,
        /// Source slice length.
        size: LiftedDataWord,
    },
    /// No recognized format; recovered segments remain available unchanged.
    Raw,
}
#[derive(Clone, Copy)]
enum Byte {
    Known(u8),
    Source(EntityId, u8),
}
impl LiftedRevertData {
    /// Decode an admitted byte expression under a bounded caller work payer.
    pub fn decode_format(
        &self,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<LiftedRevertFormat, E> {
        if !pay(self.record_count()) {
            return Err(E::Limit);
        }
        self.validate().map_err(|_| E::MissingWitness(self.id))?;
        let LiftedRevertDataKind::Segments(parts) = &self.content else {
            return Ok(LiftedRevertFormat::Raw);
        };
        if let [
            LiftedByteSegment::Slice {
                source: source @ LiftedByteSource::ReturnData(_),
                offset,
                size,
            },
        ] = parts.as_slice()
        {
            return Ok(LiftedRevertFormat::Forwarded {
                source: *source,
                offset: *offset,
                size: *size,
            });
        }
        let mut bytes = Vec::new();
        for part in parts {
            let size = match part {
                LiftedByteSegment::Bytes(b) => b.len(),
                LiftedByteSegment::Word { size, .. } => usize::from(*size),
                _ => return Ok(LiftedRevertFormat::Raw),
            };
            if bytes.len().saturating_add(size) > crate::MAX_REVERT_DATA_BYTES {
                return Ok(LiftedRevertFormat::Raw);
            }
            if !pay(size.saturating_mul(4).saturating_add(1)) {
                return Err(E::Limit);
            }
            match part {
                LiftedByteSegment::Bytes(data) => {
                    bytes.extend(data.iter().copied().map(Byte::Known))
                }
                LiftedByteSegment::Word {
                    value,
                    offset,
                    size,
                } => {
                    let range = *offset..offset.saturating_add(*size);
                    match value {
                        LiftedDataWord::Constant(value) => {
                            let data = value.to_be_bytes::<32>();
                            bytes.extend(range.map(|i| Byte::Known(data[usize::from(i)])));
                        }
                        LiftedDataWord::SourceValue(id) => {
                            bytes.extend(range.map(|i| Byte::Source(*id, i)))
                        }
                    }
                }
                _ => return Ok(LiftedRevertFormat::Raw),
            }
        }
        if bytes.is_empty() {
            return Ok(LiftedRevertFormat::Empty);
        }
        let Some(selector) = bytes.get(..4).and_then(|b| {
            b.iter()
                .map(|b| {
                    if let Byte::Known(b) = b {
                        Some(*b)
                    } else {
                        None
                    }
                })
                .collect::<Option<Vec<_>>>()
        }) else {
            return Ok(LiftedRevertFormat::Raw);
        };
        let selector: [u8; 4] = selector
            .try_into()
            .map_err(|_| E::MissingWitness(self.id))?;
        let word = |data: &[Byte]| -> Option<LiftedDataWord> {
            if data.len() != 32 {
                return None;
            }
            if let Byte::Source(id, 0) = data[0]
                && data.iter().enumerate().all(
                    |(i, b)| matches!(b,Byte::Source(other,n) if *other==id && usize::from(*n)==i),
                )
            {
                return Some(LiftedDataWord::SourceValue(id));
            }
            let mut out = [0_u8; 32];
            for (to, from) in out.iter_mut().zip(data) {
                let Byte::Known(value) = from else {
                    return None;
                };
                *to = *value;
            }
            Some(LiftedDataWord::Constant(U256::from_be_bytes(out)))
        };
        if selector == [0x08, 0xc3, 0x79, 0xa0] {
            if bytes.len() < 68
                || word(&bytes[4..36]) != Some(LiftedDataWord::Constant(U256::from(32)))
            {
                return Ok(LiftedRevertFormat::Raw);
            }
            let Some(LiftedDataWord::Constant(size)) = word(&bytes[36..68]) else {
                return Ok(LiftedRevertFormat::Raw);
            };
            let Ok(size) = usize::try_from(size) else {
                return Ok(LiftedRevertFormat::Raw);
            };
            let Some(total) = size
                .checked_add(31)
                .and_then(|n| (n / 32).checked_mul(32))
                .and_then(|n| n.checked_add(68))
            else {
                return Ok(LiftedRevertFormat::Raw);
            };
            if total != bytes.len()
                || !bytes[68 + size..]
                    .iter()
                    .all(|b| matches!(b, Byte::Known(0)))
            {
                return Ok(LiftedRevertFormat::Raw);
            }
            let message = bytes[68..68 + size]
                .iter()
                .map(|b| {
                    if let Byte::Known(b) = b {
                        Some(*b)
                    } else {
                        None
                    }
                })
                .collect::<Option<Vec<_>>>();
            return Ok(message
                .and_then(|m| String::from_utf8(m).ok())
                .map_or(LiftedRevertFormat::Raw, LiftedRevertFormat::ErrorString));
        }
        if (bytes.len() - 4) % 32 != 0 {
            return Ok(LiftedRevertFormat::Raw);
        }
        let Some(words) = bytes[4..]
            .as_chunks::<32>()
            .0
            .iter()
            .map(|part| word(part))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(LiftedRevertFormat::Raw);
        };
        if selector == [0x4e, 0x48, 0x7b, 0x71] {
            return Ok(if words.len() == 1 {
                LiftedRevertFormat::Panic(words[0])
            } else {
                LiftedRevertFormat::Raw
            });
        }
        // Unknown selectors are a lossless selector/word view, not a guessed
        // ABI. Preserve actual complete word stores; arbitrary byte arrays or
        // overwritten fragments provide no argument-boundary evidence.
        let mut prefix = 0_usize;
        for part in parts {
            if prefix < 4 {
                let n = match part {
                    LiftedByteSegment::Bytes(bytes) => bytes.len(),
                    LiftedByteSegment::Word { size, .. } => usize::from(*size),
                    _ => return Ok(LiftedRevertFormat::Raw),
                };
                prefix = prefix.saturating_add(n);
                if prefix > 4 {
                    return Ok(LiftedRevertFormat::Raw);
                }
            } else if !matches!(
                part,
                LiftedByteSegment::Word {
                    offset: 0,
                    size: 32,
                    ..
                }
            ) {
                return Ok(LiftedRevertFormat::Raw);
            }
        }
        Ok(LiftedRevertFormat::SelectorWords { selector, words })
    }
}

impl LiftedRevertFormat {
    /// Stable display label, independent of error names or selector databases.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::ErrorString(_) => "Error(string)",
            Self::Panic(_) => "Panic(uint256)",
            Self::SelectorWords { .. } => "Selector + words",
            Self::Forwarded { .. } => "Forwarded returndata",
            Self::Raw => "Unrecognized format",
        }
    }
    /// Non-semantic description for a known Solidity panic code.
    pub fn panic_explanation(&self) -> Option<&'static str> {
        let Self::Panic(LiftedDataWord::Constant(code)) = self else {
            return None;
        };
        match u64::try_from(*code).ok()? {
            0x00 => Some("编译器插入的通用 panic"),
            0x01 => Some("断言失败"),
            0x11 => Some("算术溢出或下溢"),
            0x12 => Some("除法或取模的除数为零"),
            0x21 => Some("无效枚举值"),
            0x22 => Some("存储字节数组编码无效"),
            0x31 => Some("空数组 pop"),
            0x32 => Some("数组越界"),
            0x41 => Some("内存分配过大"),
            0x51 => Some("未初始化内部函数"),
            _ => None,
        }
    }
}

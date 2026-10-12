//! Exact native array encoding for cvc5's literal-only constant-array API.
//!
//! Queries with a symbolic default use one native BV1 array per element bit.
//! A symbolic constant bit is an ITE between native all-zero/all-one arrays.
//! Reads concatenate bits, writes extract bits, and equality compares all bit
//! arrays. This is a bijective representation of the original array domain;
//! it preserves extensionality without quantifiers or index enumeration.
//! Other queries retain a single native array per original array term.

use super::{Cvc5, Native, adapter_error, native_error};
use crate::{
    Bool, Bv,
    backend::{Failure, checkpoint},
    term::{Node, Op, Sort},
};
use ::cvc5::{Kind, Term, TermManager};
use std::collections::BTreeSet;

pub(super) fn needs_bits(assertions: &[Bool], value: Option<&Bv>) -> Result<bool, Failure> {
    let mut pending: Vec<_> = assertions.iter().map(|assertion| &assertion.0).collect();
    pending.extend(value.map(|value| &value.0));
    let mut seen = BTreeSet::new();
    while let Some(term) = pending.pop() {
        checkpoint()?;
        if !seen.insert(term.id()) {
            continue;
        }
        if matches!(term.0.op, Op::ConstantArray) && !matches!(term.0.args[0].0.op, Op::Literal(_))
        {
            return Ok(true);
        }
        for child in &term.0.args {
            checkpoint()?;
            pending.push(child);
        }
    }
    Ok(false)
}

pub(super) struct BitArrays {
    remaining: u32,
}
impl BitArrays {
    pub(super) fn new(rlimit: u32) -> Self {
        Self { remaining: rlimit }
    }

    fn spend(&mut self) -> Result<(), Failure> {
        checkpoint()?;
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(Failure::ResourceLimit)?;
        Ok(())
    }

    fn term(
        &mut self,
        terms: &mut TermManager,
        kind: Kind,
        args: &[Term],
    ) -> Result<Term, Failure> {
        self.spend()?;
        Ok(terms.mk_term(kind, args).map_err(native_error)?)
    }

    fn extract(
        &mut self,
        terms: &mut TermManager,
        value: &Term,
        bit: u32,
    ) -> Result<Term, Failure> {
        self.spend()?;
        let op = terms
            .mk_op(Kind::BitvectorExtract, &[bit, bit])
            .map_err(native_error)?;
        Ok(terms
            .mk_term_from_op(op, std::slice::from_ref(value))
            .map_err(native_error)?)
    }

    fn variable(
        &mut self,
        terms: &mut TermManager,
        index_width: u32,
        element_width: u32,
        name: &str,
    ) -> Result<Native, Failure> {
        if name.contains('\0') {
            return Err(adapter_error("symbol name contains a NUL byte").into());
        }
        let index = terms.mk_bv_sort(index_width).map_err(native_error)?;
        let bit = terms.mk_bv_sort(1).map_err(native_error)?;
        let sort = terms.mk_array_sort(index, bit).map_err(native_error)?;
        let bits = (0..element_width)
            .map(|bit| {
                self.spend()?;
                Ok(terms
                    .mk_const(sort.clone(), &format!("{name}!bit!{bit}"))
                    .map_err(native_error)?)
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        Ok(Native::Bits(bits.into()))
    }

    fn constant(
        &mut self,
        terms: &mut TermManager,
        index_width: u32,
        element_width: u32,
        value: &Term,
    ) -> Result<Native, Failure> {
        let index = terms.mk_bv_sort(index_width).map_err(native_error)?;
        let bit = terms.mk_bv_sort(1).map_err(native_error)?;
        let sort = terms.mk_array_sort(index, bit).map_err(native_error)?;
        self.spend()?;
        let zero = terms.mk_bv_from_str(1, "0", 2).map_err(native_error)?;
        self.spend()?;
        let one = terms.mk_bv_from_str(1, "1", 2).map_err(native_error)?;
        self.spend()?;
        let zeros = terms
            .mk_const_array(sort.clone(), zero)
            .map_err(native_error)?;
        self.spend()?;
        let ones = terms
            .mk_const_array(sort, one.clone())
            .map_err(native_error)?;
        let bits = (0..element_width)
            .map(|bit| {
                let bit = self.extract(terms, value, bit)?;
                let condition = self.term(terms, Kind::Equal, &[bit, one.clone()])?;
                self.term(terms, Kind::Ite, &[condition, ones.clone(), zeros.clone()])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Native::Bits(bits.into()))
    }

    fn select(
        &mut self,
        terms: &mut TermManager,
        bits: &[Term],
        index: &Term,
    ) -> Result<Native, Failure> {
        let values = bits
            .iter()
            .rev()
            .map(|bit| self.term(terms, Kind::Select, &[bit.clone(), index.clone()]))
            .collect::<Result<Vec<_>, _>>()?;
        let value = if values.len() == 1 {
            values[0].clone()
        } else {
            self.term(terms, Kind::BitvectorConcat, &values)?
        };
        Ok(Native::Term(value))
    }

    fn store(
        &mut self,
        terms: &mut TermManager,
        bits: &[Term],
        index: &Term,
        value: &Term,
    ) -> Result<Native, Failure> {
        let bits = bits
            .iter()
            .enumerate()
            .map(|(index_bit, array)| {
                let bit = self.extract(
                    terms,
                    value,
                    u32::try_from(index_bit).expect("BV element width"),
                )?;
                self.term(terms, Kind::Store, &[array.clone(), index.clone(), bit])
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Native::Bits(bits.into()))
    }

    fn equal(
        &mut self,
        terms: &mut TermManager,
        a: &[Term],
        b: &[Term],
    ) -> Result<Native, Failure> {
        let equalities = a
            .iter()
            .zip(b)
            .map(|(a, b)| self.term(terms, Kind::Equal, &[a.clone(), b.clone()]))
            .collect::<Result<Vec<_>, _>>()?;
        let equality = if equalities.len() == 1 {
            equalities[0].clone()
        } else {
            self.term(terms, Kind::And, &equalities)?
        };
        Ok(Native::Term(equality))
    }

    fn ite(
        &mut self,
        terms: &mut TermManager,
        condition: &Term,
        yes: &[Term],
        no: &[Term],
    ) -> Result<Native, Failure> {
        let bits = yes
            .iter()
            .zip(no)
            .map(|(yes, no)| {
                self.term(
                    terms,
                    Kind::Ite,
                    &[condition.clone(), yes.clone(), no.clone()],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Native::Bits(bits.into()))
    }
}

impl Cvc5 {
    pub(super) fn make_bits(
        &mut self,
        node: &Node,
        args: &[Native],
    ) -> Result<Option<Native>, Failure> {
        let Some(arrays) = self.bit_arrays.as_mut() else {
            return Ok(None);
        };
        let native = match &node.op {
            Op::Variable(name) => {
                let Sort::Array {
                    index_width,
                    element_width,
                } = node.sort
                else {
                    return Ok(None);
                };
                arrays.variable(&mut self.terms, index_width, element_width, name)?
            }
            Op::ConstantArray => {
                let Sort::Array {
                    index_width,
                    element_width,
                } = node.sort
                else {
                    unreachable!("typed array")
                };
                arrays.constant(&mut self.terms, index_width, element_width, args[0].term())?
            }
            Op::Select => arrays.select(&mut self.terms, args[0].bits(), args[1].term())?,
            Op::Store => arrays.store(
                &mut self.terms,
                args[0].bits(),
                args[1].term(),
                args[2].term(),
            )?,
            Op::Eq if matches!(args[0], Native::Bits(_)) => {
                arrays.equal(&mut self.terms, args[0].bits(), args[1].bits())?
            }
            Op::Ite if matches!(node.sort, Sort::Array { .. }) => arrays.ite(
                &mut self.terms,
                args[0].term(),
                args[1].bits(),
                args[2].bits(),
            )?,
            _ => return Ok(None),
        };
        Ok(Some(native))
    }
}

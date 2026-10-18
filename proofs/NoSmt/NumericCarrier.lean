import NoSmt.ReturnLengths

set_option autoImplicit false

/-!
Exact finite-component removal and redundant restoration under the same current
remaining constraints. Context can contain a valuation, guards and unchanged
components. Completeness of the finite interval and containment of the current
remaining constraints are explicit premises, not a verified Rust detector.
-/
namespace NoSmt.NumericCarrier

universe u

abbrev Word := ReturnLengths.Word

/-- Linear unsigned closed interval; the comparisons do not wrap modulo 2^256. -/
def unsignedClosedInterval (lower upper value : Word) : Prop :=
  lower.toNat ≤ value.toNat ∧ value.toNat ≤ upper.toNat

/-- The same pointwise equivalence justifies compaction and redundant restoration. -/
theorem dense_finite_Gamma_iff {Context : Type u} (F : List Word)
    (lower upper : Word) (R : Context → Word → Prop)
    (completeInterval : ∀ value, value ∈ F ↔ unsignedClosedInterval lower upper value)
    (remainingWithin : ∀ context value, R context value →
      unsignedClosedInterval lower upper value) :
    ∀ context value, (value ∈ F ∧ R context value) ↔ R context value := by
  intro context value
  constructor
  · intro holds
    exact holds.2
  · intro holds
    exact ⟨(completeInterval value).mpr (remainingWithin context value holds), holds⟩

#print axioms dense_finite_Gamma_iff

end NoSmt.NumericCarrier

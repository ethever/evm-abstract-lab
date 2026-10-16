import NoSmt.Facts

open NoSmt.Facts

/-! Deliberately rejected: the old must-fact join excludes a permitted right-path zero.
check.sh requires this particular false proposition to fail, after checking Facts.
-/
theorem oldJoin_accepts_zero :
    accepts (lookup (oldJoin oldLeft oldRight) 7) (rightWitness 7) := by
  change (0 : Nat) ∈ ([1] : List Nat)
  decide

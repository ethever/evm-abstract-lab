import Std

set_option autoImplicit false

/-! Finite partial maps of numeric must facts. An absent entry denotes Top.
Association lists use first-match lookup (shadowed entries are immaterial).
Numeric lists denote finite sets; concatenation implements their sound union.
This does not assume that equal symbolic roots have equal operand maps.
-/
namespace NoSmt.Facts

abbrev Key := Nat
abbrev Numeric := List Nat
abbrev FactMap := List (Key × Numeric)
abbrev Valuation := Key → Nat

def lookup : FactMap → Key → Option Numeric
  | [], _ => none
  | (k, v) :: rest, query => if query = k then some v else lookup rest query

def accepts (fact : Option Numeric) (value : Nat) : Prop :=
  match fact with
  | none => True
  | some values => value ∈ values

def Gamma (facts : FactMap) (valuation : Valuation) : Prop :=
  ∀ key, accepts (lookup facts key) (valuation key)

def mergeFact : Option Numeric → Option Numeric → Option Numeric
  | some left, some right => some (left ++ right)
  | _, _ => none

/-- Keep a guarantee only when both inputs supply one for the same key. -/
def join : FactMap → FactMap → FactMap
  | [], _ => []
  | (key, value) :: rest, right =>
    match lookup right key with
    | none => join rest right
    | some other => (key, value ++ other) :: join rest right

theorem lookup_join (left right : FactMap) (key : Key) :
    lookup (join left right) key = mergeFact (lookup left key) (lookup right key) := by
  induction left with
  | nil => simp [join, lookup, mergeFact]
  | cons entry rest ih =>
    obtain ⟨k, v⟩ := entry
    by_cases h : key = k
    · subst key
      cases hr : lookup right k <;> simp [join, lookup, mergeFact, hr, ih]
    · cases hr : lookup right k <;> simp [join, lookup, mergeFact, hr, h, ih]

theorem mergeFact_left (left right : Option Numeric) (value : Nat)
    (h : accepts left value) : accepts (mergeFact left right) value := by
  cases left <;> cases right <;> simp_all [accepts, mergeFact]

theorem mergeFact_right (left right : Option Numeric) (value : Nat)
    (h : accepts right value) : accepts (mergeFact left right) value := by
  cases left <;> cases right <;> simp_all [accepts, mergeFact]

theorem join_left (left right : FactMap) (valuation : Valuation)
    (h : Gamma left valuation) : Gamma (join left right) valuation := by
  intro key
  rw [lookup_join]
  exact mergeFact_left _ _ _ (h key)

theorem join_right (left right : FactMap) (valuation : Valuation)
    (h : Gamma right valuation) : Gamma (join left right) valuation := by
  intro key
  rw [lookup_join]
  exact mergeFact_right _ _ _ (h key)

theorem missing_right_is_top (left right : FactMap) (key : Key)
    (h : lookup right key = none) : lookup (join left right) key = none := by
  rw [lookup_join, h]
  cases lookup left key <;> rfl

theorem missing_left_is_top (left right : FactMap) (key : Key)
    (h : lookup left key = none) : lookup (join left right) key = none := by
  rw [lookup_join, h]
  rfl

/-- The old clone-left/union-support policy, including numeric union on collision. -/
def oldJoin : FactMap → FactMap → FactMap
  | [], right => right
  | (key, value) :: rest, right =>
    (key, match lookup right key with
      | none => value
      | some other => value ++ other) :: oldJoin rest right

def oldLeft : FactMap := [(7, [1])]
def oldRight : FactMap := []
def rightWitness : Valuation := fun _ => 0

theorem oldJoin_counterexample :
    Gamma oldRight rightWitness ∧ ¬ Gamma (oldJoin oldLeft oldRight) rightWitness := by
  constructor
  · intro key
    trivial
  · intro h
    have bad := h 7
    simp [oldJoin, oldLeft, oldRight, rightWitness, accepts, lookup] at bad

theorem correctedWitness : Gamma (join oldLeft oldRight) rightWitness := by
  intro key
  trivial

/-- A root identity is deliberately separate from the facts interpreting it. -/
structure Rooted where
  root : Nat
  facts : FactMap
  deriving DecidableEq

theorem sameRoot_differentFacts :
    (Rooted.mk 19 oldLeft).root = (Rooted.mk 19 oldRight).root ∧
    (Rooted.mk 19 oldLeft).facts ≠ (Rooted.mk 19 oldRight).facts := by
  decide

#print axioms lookup_join
#print axioms mergeFact_left
#print axioms mergeFact_right
#print axioms join_left
#print axioms join_right
#print axioms missing_right_is_top
#print axioms missing_left_is_top
#print axioms oldJoin_counterexample
#print axioms correctedWitness
#print axioms sameRoot_differentFacts

end NoSmt.Facts

import Std

set_option autoImplicit false

/-!
The exact finite-map support-intersection algorithm, parametrized only by the
numeric component join. Its two component inclusion premises are explicit proof
obligations, not axioms. Facts.lean supplies a concrete executable finite-set
instance. Production's richer numeric component still needs a refinement proof.
-/
namespace NoSmt.PartialFacts

universe u v
variable {D : Type u} {C : Type v}

abbrev FactMap (D : Type u) := List (Nat × D)

def lookup : FactMap D → Nat → Option D
  | [], _ => none
  | (key, value) :: rest, query =>
    if query = key then some value else lookup rest query

def accepts (gamma : D → C → Prop) (fact : Option D) (value : C) : Prop :=
  match fact with
  | none => True
  | some domain => gamma domain value

def Gamma (gamma : D → C → Prop) (facts : FactMap D) (valuation : Nat → C) : Prop :=
  ∀ key, accepts gamma (lookup facts key) (valuation key)

def mergeFact (merge : D → D → D) : Option D → Option D → Option D
  | some left, some right => some (merge left right)
  | _, _ => none

def join (merge : D → D → D) : FactMap D → FactMap D → FactMap D
  | [], _ => []
  | (key, value) :: rest, right =>
    match lookup right key with
    | none => join merge rest right
    | some other => (key, merge value other) :: join merge rest right

theorem lookup_join (merge : D → D → D) (left right : FactMap D) (key : Nat) :
    lookup (join merge left right) key =
      mergeFact merge (lookup left key) (lookup right key) := by
  induction left with
  | nil => simp [join, lookup, mergeFact]
  | cons entry rest ih =>
    obtain ⟨k, v⟩ := entry
    by_cases equal : key = k
    · subst key
      cases h : lookup right k <;> simp [join, lookup, mergeFact, h, ih]
    · cases h : lookup right k <;> simp [join, lookup, mergeFact, h, equal, ih]

theorem join_left (gamma : D → C → Prop) (merge : D → D → D)
    (component_left : ∀ a b v, gamma a v → gamma (merge a b) v)
    (left right : FactMap D) (valuation : Nat → C) (h : Gamma gamma left valuation) :
    Gamma gamma (join merge left right) valuation := by
  intro key
  rw [lookup_join]
  have member := h key
  cases hl : lookup left key <;> cases hr : lookup right key <;>
    simp_all [accepts, mergeFact]

theorem join_right (gamma : D → C → Prop) (merge : D → D → D)
    (component_right : ∀ a b v, gamma b v → gamma (merge a b) v)
    (left right : FactMap D) (valuation : Nat → C) (h : Gamma gamma right valuation) :
    Gamma gamma (join merge left right) valuation := by
  intro key
  rw [lookup_join]
  have member := h key
  cases hl : lookup left key <;> cases hr : lookup right key <;>
    simp_all [accepts, mergeFact]

theorem common_support (merge : D → D → D) (left right : FactMap D) (key : Nat) :
    (lookup (join merge left right) key).isSome =
      ((lookup left key).isSome && (lookup right key).isSome) := by
  rw [lookup_join]
  cases lookup left key <;> cases lookup right key <;> rfl

theorem missing_either_is_top (merge : D → D → D) (left right : FactMap D) (key : Nat)
    (missing : lookup left key = none ∨ lookup right key = none) :
    lookup (join merge left right) key = none := by
  rw [lookup_join]
  rcases missing with missing | missing
  · rw [missing]
    rfl
  · rw [missing]
    cases lookup left key <;> rfl

/-- Discharges the component premises for a concrete union, with no assumption. -/
theorem finite_set_instance (left right : FactMap (List Nat)) (valuation : Nat → Nat) :
    (Gamma (fun values value => value ∈ values) left valuation →
      Gamma (fun values value => value ∈ values) (join List.append left right) valuation) ∧
    (Gamma (fun values value => value ∈ values) right valuation →
      Gamma (fun values value => value ∈ values) (join List.append left right) valuation) := by
  constructor
  · exact join_left _ _ (by intro a b v h; exact List.mem_append_left b h) _ _ _
  · exact join_right _ _ (by intro a b v h; exact List.mem_append_right a h) _ _ _

#print axioms lookup_join
#print axioms join_left
#print axioms join_right
#print axioms common_support
#print axioms missing_either_is_top
#print axioms finite_set_instance

end NoSmt.PartialFacts

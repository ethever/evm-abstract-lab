import NoSmt.ReturnLengths

set_option autoImplicit false

/-!
Exact pointwise factoring of branch meanings under independent Word256 guards.
The same ITE selects both branch values and derived Word256 guards: zero is
false and every nonzero word is true, as in ReturnLengths.wordTruth.
These equalities do not establish Rust state identity, metadata preservation,
expression/source coherence, or admission under production resource limits.
-/
namespace NoSmt.ExactIte

universe u v

abbrev Word256 := ReturnLengths.Word

def wordIte {Value : Type v} (guard : Word256) (left right : Value) : Value :=
  if ReturnLengths.wordTruth guard then left else right

theorem factor_left_shared_left {Index : Type u} {Value : Type v}
    (s t : Word256) (A B : Index → Value) :
    ∀ index, wordIte s (wordIte t (A index) (B index)) (A index) =
      wordIte (wordIte s t 1) (A index) (B index) := by
  intro index
  have one : ReturnLengths.wordTruth (1 : Word256) = true := rfl
  cases selected : ReturnLengths.wordTruth s <;> simp [wordIte, selected, one]

theorem factor_left_shared_right {Index : Type u} {Value : Type v}
    (s t : Word256) (A B : Index → Value) :
    ∀ index, wordIte s (wordIte t (A index) (B index)) (B index) =
      wordIte (wordIte s t 0) (A index) (B index) := by
  intro index
  have zero : ReturnLengths.wordTruth (0 : Word256) = false := rfl
  cases selected : ReturnLengths.wordTruth s <;> simp [wordIte, selected, zero]

theorem factor_right_shared_left {Index : Type u} {Value : Type v}
    (s t : Word256) (A B : Index → Value) :
    ∀ index, wordIte s (A index) (wordIte t (A index) (B index)) =
      wordIte (wordIte s 1 t) (A index) (B index) := by
  intro index
  have one : ReturnLengths.wordTruth (1 : Word256) = true := rfl
  cases selected : ReturnLengths.wordTruth s <;> simp [wordIte, selected, one]

theorem factor_right_shared_right {Index : Type u} {Value : Type v}
    (s t : Word256) (A B : Index → Value) :
    ∀ index, wordIte s (B index) (wordIte t (A index) (B index)) =
      wordIte (wordIte s 0 t) (A index) (B index) := by
  intro index
  have zero : ReturnLengths.wordTruth (0 : Word256) = false := rfl
  cases selected : ReturnLengths.wordTruth s <;> simp [wordIte, selected, zero]

theorem factor_both {Index : Type u} {Value : Type v}
    (s g h : Word256) (A B : Index → Value) :
    ∀ index, wordIte s (wordIte g (A index) (B index)) (wordIte h (A index) (B index)) =
      wordIte (wordIte s g h) (A index) (B index) := by
  intro index
  cases selected : ReturnLengths.wordTruth s <;> simp [wordIte, selected]

#print axioms factor_left_shared_left
#print axioms factor_left_shared_right
#print axioms factor_right_shared_left
#print axioms factor_right_shared_right
#print axioms factor_both

end NoSmt.ExactIte

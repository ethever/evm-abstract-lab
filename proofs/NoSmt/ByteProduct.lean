import NoSmt.ReturnLengths

set_option autoImplicit false

/-!
An unreduced intersection of component postimages is sound when every component
transfer is sound for the same concrete operation and input. Those component
obligations are premises; no production numeric transfer is verified here.
The separate Word256 results bind BYTE and AND255 shift/mask operations to byte
extraction and natural division/modulo. The Rust U256 binding remains manual.
-/
namespace NoSmt.ByteProduct

universe u v w

abbrev Word := ReturnLengths.Word

def Gamma {Index : Type u} {Concrete : Type v}
    (components : Index → Concrete → Prop) (value : Concrete) : Prop :=
  ∀ component, components component value

def Postimage {Input : Type v} {Output : Type w}
    (operation : Input → Output) (pre : Input → Prop) (output : Output) : Prop :=
  ∃ input, pre input ∧ operation input = output

/-- Each premise concerns one component, evaluated at the same concrete input.
    Input can be an operand tuple, or a scalar with literal operands fixed in
    operation. No cross-component reduction or normal-form premise is needed. -/
theorem product_transfer_sound {Index : Type u} {Input : Type v} {Output : Type w}
    (operation : Input → Output) (pre : Index → Input → Prop)
    (post : Index → Output → Prop)
    (componentSound : ∀ component input,
      pre component input → post component (operation input))
    (input : Input) (admitted : Gamma pre input) :
    Gamma post (operation input) := by
  intro component
  exact componentSound component input (admitted component)

/-- Inclusion of the actual concrete image in the unreduced output product;
    this does not assert equality, exact precision, or any Rust algorithm. -/
theorem product_postimage_inclusion {Index : Type u} {Input : Type v} {Output : Type w}
    (operation : Input → Output) (pre : Index → Input → Prop)
    (post : Index → Output → Prop)
    (componentSound : ∀ component input,
      pre component input → post component (operation input)) :
    ∀ output, Postimage operation (Gamma pre) output → Gamma post output := by
  intro output witness
  rcases witness with ⟨input, admitted, rfl⟩
  exact product_transfer_sound operation pre post componentSound input admitted

/-- A component with a false keep predicate becomes the unconstrained Top.
    This inclusion also applies to forgetting finite or nonzero constraints. -/
theorem forget_components_inclusion {Index : Type u} {Concrete : Type v}
    (components : Index → Concrete → Prop) (keep : Index → Prop)
    (value : Concrete) (admitted : Gamma components value) :
    Gamma (fun component concrete => keep component → components component concrete) value := by
  intro component _
  exact admitted component

def and255 (value : Word) : Word :=
  value &&& BitVec.ofNat 256 255

/-- Big-endian BYTE: index zero selects bits 248..255. -/
def byte (index value : Word) : Word :=
  if index.toNat < 32 then
    and255 (value >>> (8 * (31 - index.toNat)))
  else 0

def byteReference (index value : Word) : Nat :=
  if index.toNat < 32 then
    (value.toNat / 2 ^ (8 * (31 - index.toNat))) % 256
  else 0

theorem and255_toNat (value : Word) :
    (and255 value).toNat = value.toNat % 256 := by
  rw [and255, BitVec.toNat_and]
  change value.toNat &&& (2 ^ 8 - 1) = value.toNat % 2 ^ 8
  exact Nat.and_two_pow_sub_one_eq_mod value.toNat 8

private theorem and255_shift_eq_extract (value : Word) (offset : Nat) :
    and255 (value >>> offset) = (value.extractLsb' offset 8).zeroExtend 256 := by
  apply BitVec.eq_of_toNat_eq
  rw [and255_toNat, BitVec.toNat_ushiftRight,
    BitVec.toNat_setWidth_of_le (by decide : 8 ≤ 256), BitVec.extractLsb'_toNat]

theorem and255_eq_lowByte (value : Word) :
    and255 value = (value.extractLsb' 0 8).zeroExtend 256 := by
  calc
    and255 value = and255 (value >>> (0 : Nat)) :=
      congrArg and255 (BitVec.ushiftRight_zero value).symm
    _ = (value.extractLsb' 0 8).zeroExtend 256 := and255_shift_eq_extract value 0

theorem and255_range (value : Word) :
    0 ≤ (and255 value).toNat ∧ (and255 value).toNat ≤ 255 := by
  rw [and255_toNat]
  exact ⟨Nat.zero_le _, Nat.le_of_lt_succ (Nat.mod_lt _ (by decide : 0 < 256))⟩

/-- Connects the Word256 shift/mask operation to the arithmetic reference. -/
theorem byte_toNat_reference (index value : Word) :
    (byte index value).toNat = byteReference index value := by
  unfold byte byteReference
  by_cases inside : index.toNat < 32
  · simp only [if_pos inside]
    rw [and255_toNat, BitVec.toNat_ushiftRight, Nat.shiftRight_eq_div_pow]
  · simp only [if_neg inside]
    rfl

theorem byte_in_range_eq_extract (index value : Word) (inside : index.toNat < 32) :
    byte index value =
      (value.extractLsb' (8 * (31 - index.toNat)) 8).zeroExtend 256 := by
  rw [byte, if_pos inside]
  exact and255_shift_eq_extract value (8 * (31 - index.toNat))

theorem byte_out_of_range (index value : Word) (outside : 32 ≤ index.toNat) :
    byte index value = 0 := by
  exact if_neg (Nat.not_lt.mpr outside)

theorem byte_zero_is_top (value : Word) :
    byte 0 value = (value.extractLsb' 248 8).zeroExtend 256 := by
  exact byte_in_range_eq_extract 0 value (by decide)

theorem byte_31_is_low (value : Word) :
    byte 31 value = and255 value := by
  change and255 (value >>> (0 : Nat)) = and255 value
  rw [BitVec.ushiftRight_zero]

theorem byte_range (index value : Word) :
    0 ≤ (byte index value).toNat ∧ (byte index value).toNat ≤ 255 := by
  rw [byte_toNat_reference]
  by_cases inside : index.toNat < 32
  · rw [byteReference, if_pos inside]
    exact ⟨Nat.zero_le _, Nat.le_of_lt_succ (Nat.mod_lt _ (by decide : 0 < 256))⟩
  · rw [byteReference, if_neg inside]
    exact ⟨Nat.zero_le _, Nat.zero_le _⟩

#print axioms product_transfer_sound
#print axioms product_postimage_inclusion
#print axioms forget_components_inclusion
#print axioms and255_toNat
#print axioms and255_eq_lowByte
#print axioms and255_range
#print axioms byte_toNat_reference
#print axioms byte_in_range_eq_extract
#print axioms byte_out_of_range
#print axioms byte_zero_is_top
#print axioms byte_31_is_low
#print axioms byte_range

end NoSmt.ByteProduct

namespace Formal

def identity (n : Nat) : Nat := n

@[simp] theorem identity_eq (n : Nat) : identity n = n := by
  rfl

lemma unicode_identity : ∀ (p : Prop), p → p := by
  intro p h
  exact h

abbrev Count := Nat

structure Box where
  value : Nat

inductive Choice where
  | left
  | right

namespace Inner
private def nested : Nat := 1
end Inner

-- theorem Fake : False := by sorry
/- def AlsoFake := 0 -/
end Formal

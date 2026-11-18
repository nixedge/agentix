module Formal where

-- A Unicode identity.
identity : {A : Set} → A → A
identity x = x

module Inner where
  private
    nested : Set → Set
    nested A = A

data Bool : Set where
  false : Bool
  true : Bool

record Box (A : Set) : Set where
  constructor box
  field
    value : A

-- data Fake : Set where
{-
record AlsoFake : Set where
-}

_∧_ : Bool → Bool → Bool
false ∧ b = false
true ∧ b = b

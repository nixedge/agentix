module Fixture exposing (..)

identity : a -> a
identity value =
    value

type Choice
    = Left
    | Right

type alias Box =
    { value : Int }

unicode = "λ → ∀"

-- fake value = value
{- type AlsoFake = Nope -}

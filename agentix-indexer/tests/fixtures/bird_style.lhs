This module demonstrates a Bird-style Literate Haskell file.
The prose sections are plain text; each code line begins with "> ".
Bird-style is the default when no \begin{code} marker is present.

> module BirdStyle where

> -- | Add two integers together.
> addTwo :: Int -> Int -> Int
> addTwo x y = x + y

Between the two functions we have this prose explanation.
The Bird-style format requires each code line to start with "> ".
Lines without the prefix, like this one, are treated as prose by GHC.
A bare > without a trailing space is also prose (not code).

> -- | Multiply two integers together.
> mulTwo :: Int -> Int -> Int
> mulTwo x y = x * y

This final section is prose only.
There is no more code in this file.
The indexer should emit this paragraph as a documentation chunk with
doc_kind = "lhs_prose", separate from the code chunks above.

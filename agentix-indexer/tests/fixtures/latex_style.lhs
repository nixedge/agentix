\documentclass{article}
\usepackage{haskell}

\title{LaTeX-Style Literate Haskell Example}
\author{Test Fixture}

\begin{document}

This module demonstrates a LaTeX-style Literate Haskell file.
All code is enclosed in \begin{code}...\end{code} environments.
The prose between code blocks describes the implementation intent.

\begin{code}
module LatexStyle where

-- | Compute the factorial of a non-negative integer.
factorial :: Integer -> Integer
factorial 0 = 1
factorial n = n * factorial (n - 1)
\end{code}

The prose between code blocks describes the implementation.
Factorial is defined recursively: the base case is zero, which returns one.
Each subsequent recursive call reduces n by one until the base case is reached.

\begin{code}
-- | Check whether an integer is even.
isEven :: Integer -> Bool
isEven n = n `mod` 2 == 0
\end{code}

This closing section contains only prose text.
No more code blocks follow in this file.
The indexer should emit this paragraph as a documentation chunk and
associate it with no adjacent code block (none follows).

\end{document}

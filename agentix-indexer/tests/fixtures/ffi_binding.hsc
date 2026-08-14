#include <sys/types.h>
#include <stdint.h>

module FfiBinding where

-- | The file-offset type from the C standard library.
#type off_t

-- | A plain Haskell function that doubles its argument.
-- This function has no FFI dependency and is indexed as ordinary Haskell code.
double :: Int -> Int
double x = x * 2

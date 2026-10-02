module Problems where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

loop : ℕ → ℕ
loop n = loop n

partial : ℕ → ℕ
partial zero = zero

module Names where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

~>*step : ℕ → ℕ
~>*step n = suc n

one : ℕ
one = ~>*step zero

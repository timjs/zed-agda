module Helper where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

_+_ : ℕ → ℕ → ℕ
zero  + m = m
suc n + m = suc {! aux n m !}

double : ℕ → ℕ
double n = twice n
  where
    twice : ℕ → ℕ
    twice k = {! go k !}

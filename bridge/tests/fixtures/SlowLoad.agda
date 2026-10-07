module SlowLoad where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ
{-# BUILTIN NATURAL ℕ #-}

data _≡_ {A : Set} (x : A) : A → Set where
  refl : x ≡ x

ack : ℕ → ℕ → ℕ
ack zero    n       = suc n
ack (suc m) zero    = ack m (suc zero)
ack (suc m) (suc n) = ack m (ack (suc m) n)

check : ack 3 8 ≡ 2045
check = refl

again : ack 3 8 ≡ 2045
again = refl

goal : ℕ → ℕ
goal n = {!  !}

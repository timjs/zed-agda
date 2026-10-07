module Slow where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

ack : ℕ → ℕ → ℕ
ack zero    n       = suc n
ack (suc m) zero    = ack m (suc zero)
ack (suc m) (suc n) = ack m (ack (suc m) n)

three : ℕ
three = suc (suc (suc zero))

eight : ℕ
eight = suc (suc (suc (suc (suc three))))

slow : ℕ
slow = {! ack three eight !}

quick : ℕ → ℕ
quick n = {!  !}

other : ℕ → ℕ
other m = {!  !}

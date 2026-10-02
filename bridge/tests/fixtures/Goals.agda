module Goals where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

data _≡_ {A : Set} (x : A) : A → Set where
  refl : x ≡ x

_+_ : ℕ → ℕ → ℕ
n + m = {! n !}

p : zero ≡ zero
p = ?

id : {A : Set} → A → A
id x = x

two : ℕ
two = id {?} (suc (suc zero))

f : ℕ → ℕ
f = λ { x → {! x !} }

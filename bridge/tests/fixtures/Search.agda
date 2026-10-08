module Search where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

_+_ : ℕ → ℕ → ℕ
zero  + m = m
suc n + m = suc (n + m)

data _≡_ {A : Set} (x : A) : A → Set where
  refl : x ≡ x

sym : {A : Set} {x y : A} → x ≡ y → y ≡ x
sym refl = refl

trans : {A : Set} {x y z : A} → x ≡ y → y ≡ z → x ≡ z
trans refl q = q

cong : {A B : Set} (f : A → B) {x y : A} → x ≡ y → f x ≡ f y
cong f refl = refl

+-zero : (n : ℕ) → (n + zero) ≡ n
+-zero zero = refl
+-zero (suc n) = cong suc (+-zero n)

+-suc : (n m : ℕ) → (n + suc m) ≡ suc (n + m)
+-suc zero m = refl
+-suc (suc n) m = cong suc (+-suc n m)

-- Without induction, auto finds no proof; with a time limit of 10 s it
-- searches long.
comm : (n m : ℕ) → (n + m) ≡ (m + n)
comm n m = {! -t 10000 trans sym cong +-zero +-suc !}

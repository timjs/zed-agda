module Spike where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

_+_ : ℕ → ℕ → ℕ
zero  + m = m
suc n + m = {! suc (n + m) !}

double : ℕ → ℕ
double n = {!   !}

data 𝔹 : Set where
  tt ff : 𝔹

not : 𝔹 → 𝔹
not = λ (b : 𝔹) → {!   !}

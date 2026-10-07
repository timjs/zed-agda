module Depends where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

id : {A : Set} → A → A
id x = x

const : {A B : Set} → A → B → A
const a _ = a

g : ℕ
g = const zero (id {{!  !}} {!  !})

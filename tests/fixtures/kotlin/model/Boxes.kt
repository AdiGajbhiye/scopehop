package com.example.model

// A third `shape` member, in a file that sorts before Holders.kt, so taking
// the first candidate for an untyped `h.shape` would pick Square.
class SquareHolder(val shape: Square)

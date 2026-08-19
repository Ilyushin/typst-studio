#set page(width: 256pt, height: 256pt, margin: 0pt, fill: none)

#let blue = rgb("#3b82f6")
#let deep = rgb("#1e3a8a")

#place(center + horizon, block(
  width: 256pt,
  height: 256pt,
  fill: gradient.linear(deep, blue, angle: 135deg),
  radius: 56pt,
))

// A "T" drawn as blocks rather than glyphs, so the mark does not depend on
// which font happens to be available.
#place(center + horizon, dy: -18pt, rect(width: 132pt, height: 30pt, fill: white, radius: 4pt))
#place(center + horizon, dy: 26pt, rect(width: 30pt, height: 118pt, fill: white, radius: 4pt))

"""Scatola con coperchio 60x60 — fixture per la galleria di confronto."""
from build123d import *

WALL = 2.0
HEIGHT = 8.0
LID_H = 10.0

with BuildPart() as base:
    Box(60, 60, HEIGHT, align=(Align.CENTER, Align.CENTER, Align.MIN))
    offset(amount=-WALL, openings=faces().sort_by(Axis.Z)[-1])

with BuildPart() as lid:
    Box(60, 60, LID_H, align=(Align.CENTER, Align.CENTER, Align.MAX))
    offset(amount=-WALL, openings=faces().sort_by(Axis.Z)[0])

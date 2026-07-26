"""Bitwise operators, augmented assignment and unary `not` — the corners a
token-level mutator has to reach to match mutmut."""

READ = 0b0001
WRITE = 0b0010
EXECUTE = 0b0100
ALL = 0b0111


def grant(current, permission):
    return current | permission


def revoke(current, permission):
    return current & ~permission


def toggle(current, permission):
    return current ^ permission


def can(current, permission):
    return current & permission != 0


def shift_left(value, places):
    return value << places


def accumulate(values):
    total = 0
    for value in values:
        total += value
        total *= 2
    return total


def countdown(start):
    remaining = start
    steps = 0
    while remaining > 0:
        remaining -= 1
        steps += 1
        if steps > 100:
            break
    return steps


def is_blocked(user):
    return not user.get("active", True)

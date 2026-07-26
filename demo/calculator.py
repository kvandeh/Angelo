"""Arithmetic and comparison — the operators everyone expects to be mutated."""

# Module level: this runs at import time, so every test executes it.
DEFAULT_RATE = 0.21


def add(a, b):
    return a + b


def subtract(a, b):
    return a - b


def scale(value, factor):
    return value * factor


def average(values):
    return sum(values) / len(values)


def clamp(value, low, high):
    if value < low:
        return low
    if value > high:
        return high
    return value


def is_adult(age):
    return age >= 18


def in_range(value, low, high):
    return value >= low and value <= high


def with_tax(price, rate=DEFAULT_RATE):
    return price + price * rate


def final_price(price, discount_percent):
    if discount_percent > 0 and discount_percent <= 100:
        return price - price * discount_percent / 100
    return price


def describe(price):
    """Never called by any test — its mutants survive without running."""
    if price >= 100:
        return "expensive"
    return "cheap"

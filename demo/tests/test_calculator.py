from calculator import (
    add,
    average,
    clamp,
    final_price,
    in_range,
    is_adult,
    scale,
    subtract,
    with_tax,
)

# Deliberate gaps, so the demo has survivors worth looking at:
#   - clamp is never tested above `high`
#   - is_adult is never tested at exactly 18
#   - in_range is never tested on its boundaries
#   - describe() has no test at all, so its mutants never run


def test_add():
    assert add(2, 3) == 5


def test_subtract():
    assert subtract(5, 3) == 2


def test_scale():
    assert scale(3, 4) == 12


def test_average():
    assert average([2, 4, 6]) == 4


def test_clamp_below_low():
    assert clamp(1, 5, 10) == 5


def test_clamp_inside_range():
    assert clamp(7, 5, 10) == 7


def test_is_adult():
    assert is_adult(30)
    assert not is_adult(10)


def test_in_range():
    assert in_range(5, 1, 10)
    assert not in_range(50, 1, 10)


def test_with_tax():
    assert with_tax(100.0, 0.5) == 150.0


def test_final_price():
    assert final_price(100.0, 20.0) == 80.0

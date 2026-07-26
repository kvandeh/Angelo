from text import (
    find_marker,
    first_field,
    initials,
    label,
    normalise,
    shout,
    trim_prefix,
)

# trim_prefix has no test, so its mutants survive without running.


def test_normalise():
    assert normalise("  Ada  ") == "ada"


def test_shout():
    assert shout("quiet") == "QUIET"


def test_initials():
    assert initials("Ada Lovelace") == "AL"


def test_first_field():
    assert first_field("a,b,c") == "a"


def test_label():
    assert label(1) == "1 item"
    assert label(3) == "3 items"


def test_find_marker():
    assert find_marker("hello", "l") == 2

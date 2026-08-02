from flags import (
    READ,
    WRITE,
    accumulate,
    can,
    countdown,
    grant,
    is_blocked,
    revoke,
    shift_left,
    toggle,
)

# shift_left has no test. countdown is never pushed past its `break`.


def test_grant():
    assert grant(READ, WRITE) == 0b0011


def test_revoke():
    assert revoke(0b0011, WRITE) == READ


def test_toggle():
    assert toggle(READ, READ) == 0


def test_can():
    assert can(0b0011, READ)
    assert not can(0b0100, READ)


def test_accumulate():
    # ((0 + 1) * 2 + 2) * 2
    assert accumulate([1, 2]) == 8


def test_countdown():
    assert countdown(3) == 3


def test_is_blocked():
    assert is_blocked({"active": False})
    assert not is_blocked({"active": True})

"""Strings and string methods — mutants that swap case, wrap in XX, or flip
a method for its mirror image."""

SEPARATOR = ","


def normalise(name):
    return name.lower().strip()


def shout(message):
    return message.upper()


def initials(full_name):
    return "".join(part[0] for part in full_name.split(" "))


def first_field(line):
    return line.split(SEPARATOR)[0]


def trim_prefix(text, prefix):
    return text.removeprefix(prefix)


def label(count):
    if count == 1:
        return "1 item"
    return f"{count} items"


def find_marker(haystack, needle):
    return haystack.find(needle)

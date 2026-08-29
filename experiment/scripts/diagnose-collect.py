"""Turn a failed `pytest --co` log into the pip packages that would fix it.

Printed one per line, empty when nothing here can be fixed by installing. The
harness installs them and collects again, so a repo declaring nothing still
converges instead of being written off -- which is what happened to rich (494
tests collected, discarded over one `import attr`).

Usage: python diagnose-collect.py <log> [--project NAME]
"""

import argparse
import re
import sys

# Import names that differ from the package that provides them. Everything else
# converts by the usual rule: underscores become hyphens.
PACKAGE_OF = {
    "attr": "attrs",
    "yaml": "PyYAML",
    "OpenSSL": "pyOpenSSL",
    "PIL": "pillow",
    "dateutil": "python-dateutil",
    "dotenv": "python-dotenv",
    "jwt": "PyJWT",
    "pkg_resources": "setuptools",
    "win32api": "pywin32",
    "zope": "zope.interface",
}

# A pytest option only exists when its plugin is installed, so an "unrecognized
# arguments" line names a plugin the project's own addopts depends on.
PLUGIN_OF_OPTION = {
    "-n": "pytest-xdist",
    "--numprocesses": "pytest-xdist",
    "--cov": "pytest-cov",
    "--asyncio-mode": "pytest-asyncio",
    "--timeout": "pytest-timeout",
    "--benchmark": "pytest-benchmark",
    "--forked": "pytest-forked",
    "--randomly": "pytest-randomly",
    "--mypy": "pytest-mypy",
    "--flake8": "pytest-flake8",
    "--black": "pytest-black",
}

# Same, for the ini keys and markers a plugin registers.
PLUGIN_OF_SETTING = {
    "asyncio_mode": "pytest-asyncio",
    "asyncio_default_fixture_loop_scope": "pytest-asyncio",
    "xdist_group": "pytest-xdist",
    "timeout": "pytest-timeout",
    "benchmark": "pytest-benchmark",
}

# Modules that no wheel provides: they are built by the project's own test
# tooling, so installing something named after them would fetch a stranger.
NEVER_INSTALL = {"cythonapp", "conftest", "test", "tests"}


# PowerShell writes UTF-16 from some redirections and UTF-8-with-BOM from
# others, and both start with a byte-order mark that says which. Guessing by
# trying UTF-16 first does not work: UTF-8 bytes decode as UTF-16 into garbage
# that contains no NUL, so the guess looks like it succeeded.
BOMS = (
    (b"\xef\xbb\xbf", "utf-8-sig"),
    (b"\xff\xfe", "utf-16"),
    (b"\xfe\xff", "utf-16"),
)


def read(path):
    """Logs come from PowerShell redirection, so the encoding varies."""
    with open(path, "rb") as handle:
        raw = handle.read()
    for mark, encoding in BOMS:
        if raw.startswith(mark):
            return raw.decode(encoding, "replace")
    # No mark. UTF-16 without one is still obvious: ASCII text in it carries a
    # NUL beside every character, and which side says which endianness.
    if b"\x00" in raw[:200]:
        wide = "utf-16-be" if raw[:1] == b"\x00" else "utf-16-le"
        return raw.decode(wide, "replace")
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("latin-1", "replace")


def package_for(module, project):
    top = module.split(".")[0]
    if not top or top in NEVER_INSTALL:
        return None
    package = PACKAGE_OF.get(top, top.replace("_", "-"))
    # A project failing to import itself means its own build failed. Pulling the
    # same name off PyPI would paper over that with a stranger's release. Both
    # spellings have to be checked: the pyyaml repo imports `yaml`, so comparing
    # only the import name would have reinstalled it over its own broken build.
    if project:
        theirs = project.replace("_", "-").lower()
        if theirs in (top.replace("_", "-").lower(), package.lower()):
            return None
    return package


def wanted(text, project):
    found = []

    def add(name):
        if name and name not in found:
            found.append(name)

    for module in re.findall(r"No module named '([^']+)'", text):
        add(package_for(module, project))
    # A filterwarnings entry naming a module pytest cannot import fails config
    # before a single test runs.
    for module in re.findall(r"Failed to import filter module '([^']+)'", text):
        add(package_for(module, project))
    for match in re.findall(r"unrecognized arguments:\s*(.+)", text):
        for token in match.split():
            option = token.split("=")[0]
            add(PLUGIN_OF_OPTION.get(option))
    for setting in re.findall(r"Unknown config option:\s*(\w+)", text):
        add(PLUGIN_OF_SETTING.get(setting))
    for marker in re.findall(r"'(\w+)' not found in `markers`", text):
        add(PLUGIN_OF_SETTING.get(marker))
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("log")
    parser.add_argument("--project", default="")
    args = parser.parse_args()
    for package in wanted(read(args.log), args.project):
        print(package)


if __name__ == "__main__":
    sys.exit(main())

"""Tests for the two helpers the corpus harness leans on.

Both fail *silently* when they are wrong -- a mis-decoded log simply finds no
packages, and a missed dependency group simply installs nothing -- which is
exactly how 36 of 50 repos reached pytest with no test dependencies and were
written off as broken projects. Run with:

    python -m pytest experiment/scripts/test_harness_scripts.py
"""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).parent


def load(filename, name):
    """Both helpers are hyphenated, so they cannot be imported by name."""
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


diagnose = load("diagnose-collect.py", "diagnose_collect")
read_deps = load("read-deps.py", "read_deps")


# --- diagnose-collect: reading the log -------------------------------------

# PowerShell writes UTF-16 from some redirections and UTF-8-with-BOM from
# others. Trying UTF-16 first does not work, because UTF-8 bytes decode as
# UTF-16 into garbage containing no NUL -- so the guess looks successful and
# every pattern then matches nothing.
@pytest.mark.parametrize(
    "encoding",
    ["utf-8", "utf-8-sig", "utf-16", "utf-16-le", "utf-16-be"],
)
def test_a_log_is_read_whatever_powershell_encoded_it_as(tmp_path, encoding):
    log = tmp_path / "collect.log"
    log.write_text("E   ModuleNotFoundError: No module named 'trio'", encoding=encoding)

    assert diagnose.wanted(diagnose.read(str(log)), "") == ["trio"]


def test_an_undecodable_log_does_not_raise(tmp_path):
    log = tmp_path / "collect.log"
    log.write_bytes(b"\x81\x82 No module named 'trio'")

    assert "trio" in diagnose.read(str(log))


# --- diagnose-collect: what to install --------------------------------------


def test_an_import_name_maps_to_the_package_that_provides_it():
    assert diagnose.wanted("No module named 'attr'", "") == ["attrs"]
    assert diagnose.wanted("No module named 'yaml'", "") == ["PyYAML"]


def test_an_underscore_name_becomes_a_hyphen_package():
    assert diagnose.wanted("No module named 'pytest_asyncio'", "") == ["pytest-asyncio"]


def test_a_submodule_resolves_to_its_top_level_package():
    assert diagnose.wanted("No module named 'jsonpath_ng.ext'", "") == ["jsonpath-ng"]


# A project failing to import itself means its own build failed. Installing the
# same name from PyPI would paper over that with a stranger's release -- which
# is what `No module named 'cryptography'` in the cryptography repo really was.
def test_a_project_failing_to_import_itself_is_never_reinstalled():
    assert diagnose.wanted("No module named 'cryptography'", "cryptography") == []
    assert diagnose.wanted("No module named 'yaml.reader'", "pyyaml") == []


def test_a_build_artifact_is_never_fetched_from_pypi():
    assert diagnose.wanted("No module named 'cythonapp'", "tornado") == []


# An option only exists when its plugin does, so "unrecognized arguments" names
# a plugin the project's own addopts depends on.
@pytest.mark.parametrize(
    ("line", "package"),
    [
        ("pytest: error: unrecognized arguments: -n 8", "pytest-xdist"),
        ("pytest: error: unrecognized arguments: --cov=gunicorn", "pytest-cov"),
        ("pytest: error: unrecognized arguments: --asyncio-mode=strict", "pytest-asyncio"),
    ],
)
def test_an_unrecognized_option_names_its_plugin(line, package):
    assert diagnose.wanted(line, "") == [package]


def test_an_unknown_ini_key_names_its_plugin():
    text = "PytestConfigWarning: Unknown config option: asyncio_default_fixture_loop_scope"
    assert diagnose.wanted(text, "") == ["pytest-asyncio"]


def test_an_unregistered_marker_names_its_plugin():
    assert diagnose.wanted("Failed: 'xdist_group' not found in `markers`", "") == [
        "pytest-xdist"
    ]


# A filterwarnings entry naming an unimportable module fails config before a
# single test runs, and says so in wording of its own.
def test_a_broken_filterwarnings_entry_names_its_module():
    text = "PytestConfigWarning: Failed to import filter module 'trio'"
    assert diagnose.wanted(text, "") == ["trio"]


def test_nothing_installable_gives_an_empty_list():
    assert diagnose.wanted("some unrelated failure", "") == []


def test_each_package_is_named_once():
    text = "No module named 'trio'\nNo module named 'trio'\nFailed to import filter module 'trio'"
    assert diagnose.wanted(text, "") == ["trio"]


# --- read-deps: what a project declares -------------------------------------


def write_pyproject(tmp_path, body):
    (tmp_path / "pyproject.toml").write_text(body, encoding="utf-8")
    return tmp_path


def report_for(tmp_path):
    out = subprocess.run(
        [sys.executable, str(HERE / "read-deps.py"), str(tmp_path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(out.stdout)


# The bug this exists for: `pip install .[tests]` cannot reach a PEP 735 group
# at any spelling, and pip exits 0 with only a warning when the extra is absent,
# so guessing looked like success and installed nothing.
def test_a_pep_735_dependency_group_is_found(tmp_path):
    write_pyproject(
        tmp_path,
        "[dependency-groups]\ndocs = ['sphinx']\ntests = ['pytest-xdist']\n",
    )

    report = report_for(tmp_path)

    assert report["test_groups"] == ["tests"]
    assert "docs" not in report["test_groups"]


def test_the_most_test_specific_group_comes_first(tmp_path):
    write_pyproject(
        tmp_path,
        "[dependency-groups]\ndev = ['x']\ntest = ['y']\n",
    )

    assert report_for(tmp_path)["test_groups"] == ["test", "dev"]


def test_an_optional_dependency_extra_is_found(tmp_path):
    write_pyproject(
        tmp_path,
        "[project]\nname = 'x'\nversion = '1'\n"
        "[project.optional-dependencies]\ntesting = ['pytest']\nsocks = ['pysocks']\n",
    )

    report = report_for(tmp_path)

    assert report["test_extras"] == ["testing"]
    assert "socks" not in report["test_extras"]


def test_poetry_keeps_its_groups_outside_pep_735(tmp_path):
    write_pyproject(
        tmp_path,
        "[tool.poetry]\nname = 'x'\n[tool.poetry.group.dev.dependencies]\npytest = '*'\n",
    )

    assert report_for(tmp_path)["poetry_groups"] == ["dev"]


def test_requirements_files_are_found_and_ranked(tmp_path):
    write_pyproject(tmp_path, "[project]\nname = 'x'\nversion = '1'\n")
    (tmp_path / "test-requirements.txt").write_text("pytest\n", encoding="utf-8")
    requirements = tmp_path / "requirements"
    requirements.mkdir()
    # The plain one must sort ahead: test-integration.txt wants a live broker.
    (requirements / "test-integration.txt").write_text("kombu\n", encoding="utf-8")
    (requirements / "test.txt").write_text("pytest\n", encoding="utf-8")

    found = report_for(tmp_path)["requirements"]

    assert found[0] == "test-requirements.txt"
    assert found.index("requirements/test.txt") < found.index(
        "requirements/test-integration.txt"
    )


def test_a_docs_requirements_file_is_left_alone(tmp_path):
    write_pyproject(tmp_path, "[project]\nname = 'x'\nversion = '1'\n")
    requirements = tmp_path / "requirements"
    requirements.mkdir()
    (requirements / "docs.txt").write_text("sphinx\n", encoding="utf-8")

    assert report_for(tmp_path)["requirements"] == []


# A project with no pyproject at all still has to produce a usable answer,
# rather than a traceback the harness would read as "no dependencies".
def test_a_project_without_a_pyproject_reports_empty(tmp_path):
    report = report_for(tmp_path)

    assert report["test_groups"] == []
    assert report["test_extras"] == []


def test_an_unparseable_pyproject_reports_empty(tmp_path):
    write_pyproject(tmp_path, "this is not toml [[[")

    assert report_for(tmp_path)["test_groups"] == []

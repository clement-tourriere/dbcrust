"""Repo-level pytest bootstrap.

Makes the pure-Python test suite (python/dbcrust/django/tests) runnable from
a fresh checkout without building the Rust extension: `dbcrust/__init__.py`
imports the compiled `dbcrust._internal` module unconditionally, so without
this stub every test fails at collection with ModuleNotFoundError.

If the real extension is built (maturin develop / installed wheel), it is
used and the stub never activates.
"""

import sys
import types
from pathlib import Path

# Make `import dbcrust` resolve to the in-repo source tree
_PYTHON_SRC = str(Path(__file__).parent / "python")
if _PYTHON_SRC not in sys.path:
    sys.path.insert(0, _PYTHON_SRC)

try:  # Real compiled extension available? Use it.
    import dbcrust._internal  # noqa: F401
except ImportError:
    _stub = types.ModuleType("dbcrust._internal")

    class _Unavailable:
        """Placeholder for native classes when the extension isn't built."""

        def __init__(self, *args, **kwargs):
            raise RuntimeError(
                "dbcrust._internal is not built in this environment "
                "(run `maturin develop --features python` for native tests)"
            )

    for _name in (
        "PyDatabase",
        "PyConfig",
        "PyConnection",
        "PyCursor",
        "PyServerInfo",
        "PyRow",
        "PyResultSet",
    ):
        setattr(_stub, _name, type(_name, (_Unavailable,), {}))

    for _name in (
        "DbcrustError",
        "DbcrustConnectionError",
        "DbcrustCommandError",
        "DbcrustConfigError",
        "DbcrustArgumentError",
    ):
        setattr(_stub, _name, type(_name, (Exception,), {}))

    def _native_unavailable(*args, **kwargs):
        raise RuntimeError("dbcrust._internal is not built in this environment")

    _stub.run_command = _native_unavailable
    _stub.run_cli_loop = _native_unavailable
    _stub.py_connect = _native_unavailable

    sys.modules["dbcrust._internal"] = _stub

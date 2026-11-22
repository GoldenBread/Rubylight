"""Antigravity `run_workflow` entry script for the Rubylight & Nightjar 7.0.0 roadmap.

Invoke via `run_workflow`:
    ScriptPath: <repo>/.agents/workflows/roadmap_orchestrator.py
    Cwd: <repo>
    Args: {"waves": [0, 1, 2, 3, 4, 5, 6], "max_parallel": 2, "authorize_publish": false}

Wave 6 prepares 7.0.0 and runs the Whole-Release Carbon review, then stops cleanly
until explicitly re-run with `authorize_publish: true` after maintainer sign-off.
"""
import importlib.util
import pathlib
import sys

_repo_root = pathlib.Path(args.get("repo_root") or pathlib.Path.cwd()).resolve()
_core_path = _repo_root / ".agents" / "workflows" / "orchestrator_core.py"
_spec = importlib.util.spec_from_file_location("rubylight_orchestrator_core", _core_path)
if _spec is None or _spec.loader is None:
    raise RuntimeError(f"Unable to load orchestrator core at {_core_path}")
_core = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = _core
_spec.loader.exec_module(_core)

_runtime = _core.WorkflowRuntime(
    repo_root=_repo_root,
    agent=agent,
    parallel=parallel,
    pipeline=pipeline,
    phase=phase,
    log=log,
    call_tool=call_tool,
)

_summary = await _core.run_roadmap(
    _runtime,
    waves=args.get("waves", [0, 1, 2, 3, 4, 5, 6]),
    max_parallel=args.get("max_parallel", 2),
    dry_run=bool(args.get("dry_run", False)),
    authorize_publish=bool(args.get("authorize_publish", False)),
)
log("Roadmap orchestrator finished:", _summary)

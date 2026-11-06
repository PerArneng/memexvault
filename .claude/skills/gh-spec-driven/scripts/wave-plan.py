#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "pyyaml>=6.0",
# ]
# ///
"""Turn the open specs' dependencies into a dependency graph, a draft of waves, and finally real wave issues.

What it does:
  Reads open spec issues through gh and `**Depends on:**` lines from specs/NNN-name/spec.md. Hold specs are shown in the
  graph but never go in waves, because no agent can work on them. wave-manager.py must sit next to this script.
    graph            open specs and their dependencies, as a table, JSON (--json) or Graphviz (--dot)
    layers           topological levels: what could run if only dependencies mattered
    propose          a first-draft plan from dependencies plus a file of code areas; expect to edit it
    validate PLAN    check a plan: each open non-hold spec in exactly one wave, dependencies met, no cycles.
                     With --areas, warn about shared code areas inside a wave or between parallel waves.
    apply PLAN       create the plan's waves in graph order through `wave-manager.py create`
  A plan is YAML:
      exclude: [64]          # open specs deliberately left out of waves
      waves:
        - key: B1            # used only inside the plan, for `after`
          name: entities, session, TLS
          description: What this wave does
          issues: [23, 37]
          after: []
  An areas file is YAML, with one entry per spec; specs on different tracks are planned as parallel waves:
      specs:
        23: {track: backend, areas: [domain-model, search-index]}

Output:
  stdout  graph, layers: text or JSON or DOT. propose: plan YAML. validate: nothing. apply: "KEY NUMBER" for each wave.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR (one per problem found), FAIL.

Exit codes:
  0  success
  1  failure, such as a cycle or an invalid plan (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Reads issues through gh. apply creates public GitHub issues, and its --dry-run only logs. A rerun of apply reuses waves
  whose title already exists, and fails if their members differ. Everything else is read-only.

Examples:
  uv run .claude/skills/gh-spec-driven/scripts/wave-plan.py layers
  uv run .claude/skills/gh-spec-driven/scripts/wave-plan.py graph --dot | dot -Tsvg -o graph.svg
  uv run .claude/skills/gh-spec-driven/scripts/wave-plan.py propose --areas areas.yaml > plan.yaml
  uv run .claude/skills/gh-spec-driven/scripts/wave-plan.py validate plan.yaml --areas areas.yaml
  uv run .claude/skills/gh-spec-driven/scripts/wave-plan.py apply plan.yaml --dry-run

--help output:
[BEGIN HELP]
  $ wave-plan.py --help
  usage: wave-plan.py [-h] [-v] [--root DIR] {graph,layers,propose,validate,apply} ...

  Turn the open specs' dependencies into a dependency graph, a draft of waves, and
  finally real wave issues.

  positional arguments:
    {graph,layers,propose,validate,apply}
      graph               print open specs and their dependencies
      layers              print topological levels of the open specs
      propose             print a first-draft plan
      validate            check a plan
      apply               create the plan's wave issues

  options:
    -h, --help            show this help message and exit
    -v, --verbose         log DEBUG lines too (default: False)
    --root DIR            flat spec folder to read Depends on lines from (default: <git
                          repo root>/specs)

  $ wave-plan.py graph --help
  usage: wave-plan.py graph [-h] [--json] [--dot]

  options:
    -h, --help  show this help message and exit
    --json      print JSON (default: False)
    --dot       print Graphviz (default: False)

  $ wave-plan.py layers --help
  usage: wave-plan.py layers [-h] [--json]

  options:
    -h, --help  show this help message and exit
    --json      print JSON (default: False)

  $ wave-plan.py propose --help
  usage: wave-plan.py propose [-h] --areas FILE [--exclude N [N ...]]

  options:
    -h, --help           show this help message and exit
    --areas FILE         YAML file of each spec's track and code areas
    --exclude N [N ...]  open specs to leave out of waves (default: [])

  $ wave-plan.py validate --help
  usage: wave-plan.py validate [-h] [--areas FILE] PLAN

  positional arguments:
    PLAN          plan YAML file

  options:
    -h, --help    show this help message and exit
    --areas FILE  also warn about shared code areas (default: None)

  $ wave-plan.py apply --help
  usage: wave-plan.py apply [-h] [--dry-run] PLAN

  positional arguments:
    PLAN        plan YAML file

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)
[END HELP]
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import yaml
from pydantic import BaseModel, ConfigDict, ValidationError


def load_wave_manager() -> Any:
    """wave-manager.py has a dash in its name, so it's loaded by path. It owns status rules, gh and logging."""
    path = Path(__file__).resolve().parent / "wave-manager.py"
    spec = importlib.util.spec_from_file_location("wave_manager", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    sys.dont_write_bytecode = True  # keeps __pycache__ out of the skill folder
    module = importlib.util.module_from_spec(spec)
    sys.modules["wave_manager"] = module  # pydantic resolves the module's annotations through this
    spec.loader.exec_module(module)
    return module


wm = load_wave_manager()
log = wm.log
kv = wm.kv
ScriptFailure = wm.ScriptFailure


# --------------------------------------------------------------------------- models


class Node(BaseModel):
    """An open spec, with dependencies limited to other open specs."""

    model_config = ConfigDict(frozen=True)

    number: int
    name: str
    status: str
    depends: list[int]


class PlanWave(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    key: str
    name: str
    description: str
    issues: list[int]
    after: list[str] = []


class Plan(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    exclude: list[int] = []
    waves: list[PlanWave]


class AreaEntry(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    track: str
    areas: list[str]


class Areas(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    specs: dict[int, AreaEntry]


# --------------------------------------------------------------------------- pure core


def build_nodes(issues: list[Any], depends: dict[int, set[int]]) -> dict[int, Node]:
    """Open specs and their open dependencies. Closed dependencies are already met, so they drop out."""
    open_specs = {i.number: i for i in issues if wm.SPEC_LABEL in i.labels and i.state == "OPEN"}
    return {n: Node(number=n, name=wm.short_name(i), status=wm.issue_status(i),
                    depends=sorted((depends.get(n, set()) - {n}) & set(open_specs)))
            for n, i in sorted(open_specs.items())}


def workable(nodes: dict[int, Node]) -> dict[int, Node]:
    return {n: node for n, node in nodes.items() if node.status != "hold"}


def compute_layers(nodes: dict[int, Node]) -> list[list[int]]:
    """Kahn's algorithm by levels, over workable specs. Dependencies on hold specs are reported by validate instead."""
    work = workable(nodes)
    remaining = {n: set(node.depends) & set(work) for n, node in work.items()}
    layers: list[list[int]] = []
    while remaining:
        layer = sorted(n for n, deps in remaining.items() if not deps)
        if not layer:
            raise ScriptFailure(f"the specs' dependencies form a cycle {kv(involved=sorted(remaining))}")
        layers.append(layer)
        for n in layer:
            del remaining[n]
        for deps in remaining.values():
            deps.difference_update(layer)
    return layers


def propose_plan(nodes: dict[int, Node], areas: Areas, exclude: list[int]) -> Plan:
    """List scheduling: each round, every track starts one wave of ready specs that share no code area."""
    work = {n: node for n, node in workable(nodes).items() if n not in exclude}
    missing = sorted(set(work) - set(areas.specs))
    if missing:
        raise ScriptFailure(f"the areas file has no entry for these open specs {kv(specs=missing)}")
    compute_layers(nodes)  # fails on a cycle, which the loop below would otherwise never finish
    placed: dict[int, str] = {}
    last_in_track: dict[str, str] = {}
    counts: dict[str, int] = {}
    waves: list[PlanWave] = []
    tracks = sorted({areas.specs[n].track for n in work})
    while len(placed) < len(work):
        before_round = set(placed)
        for track in tracks:
            ready = [n for n in sorted(work) if n not in placed and areas.specs[n].track == track
                     and all(d in before_round or d not in work for d in work[n].depends)]
            chosen: list[int] = []
            used: set[str] = set()
            for n in ready:
                wanted = set(areas.specs[n].areas)
                if not wanted & used:
                    chosen.append(n)
                    used |= wanted
            if not chosen:
                continue
            counts[track] = counts.get(track, 0) + 1
            key = f"{track[0].upper()}{counts[track]}"
            after = {placed[d] for n in chosen for d in work[n].depends if d in placed}
            if track in last_in_track:
                after.add(last_in_track[track])
            names = ", ".join(work[n].name for n in chosen)
            waves.append(PlanWave(key=key, name=f"{track} {counts[track]}: {names}", description=f"Draft: {names}.", issues=chosen, after=sorted(after)))
            for n in chosen:
                placed[n] = key
            last_in_track[track] = key
    return Plan(exclude=sorted(exclude), waves=waves)


def plan_ancestors(plan: Plan) -> dict[str, set[str]]:
    """Each wave's ancestors, assuming keys and parents are valid and acyclic."""
    by_key = {w.key: w for w in plan.waves}
    found: dict[str, set[str]] = {}

    def visit(key: str) -> set[str]:
        if key not in found:
            result: set[str] = set()
            for parent in by_key[key].after:
                result |= {parent} | visit(parent)
            found[key] = result
        return found[key]

    for key in by_key:
        visit(key)
    return found


def plan_order(plan: Plan) -> list[PlanWave]:
    """Waves with parents first. Raises ScriptFailure on a cycle or a parent that doesn't exist."""
    by_key = {w.key: w for w in plan.waves}
    remaining = {w.key: {p for p in w.after} for w in plan.waves}
    order: list[PlanWave] = []
    for key, parents in remaining.items():
        unknown = sorted(parents - set(by_key))
        if unknown:
            raise ScriptFailure(f"a plan wave has an unknown parent {kv(wave=key, unknown=unknown)}")
    while remaining:
        ready = [k for k in by_key if k in remaining and not remaining[k]]
        if not ready:
            raise ScriptFailure(f"the plan's waves form a cycle {kv(involved=sorted(remaining))}")
        for k in ready:
            order.append(by_key[k])
            del remaining[k]
        for parents in remaining.values():
            parents.difference_update(ready)
    return order


def validate_plan(plan: Plan, nodes: dict[int, Node], areas: Areas | None) -> tuple[list[str], list[str]]:
    """Problems and warnings. Problems stop `apply`; warnings are judgment calls for a person to read."""
    problems: list[str] = []
    warnings: list[str] = []
    keys = [w.key for w in plan.waves]
    problems += [f"wave key {k} appears twice" for k in sorted({k for k in keys if keys.count(k) > 1})]
    if problems:
        return problems, warnings
    try:
        plan_order(plan)
    except ScriptFailure as e:
        return [str(e)], warnings
    owner: dict[int, str] = {}
    for w in plan.waves:
        if not w.issues:
            problems.append(f"wave {w.key}: has no issues")
        for n in w.issues:
            if n in owner:
                problems.append(f"issue #{n} is in both {owner[n]} and {w.key}")
            owner.setdefault(n, w.key)
            if n not in nodes:
                problems.append(f"wave {w.key}: #{n} isn't an open spec")
            elif nodes[n].status == "hold":
                problems.append(f"wave {w.key}: #{n} is on hold, and hold specs stay out of waves")
    for n, node in sorted(workable(nodes).items()):
        if n not in owner and n not in plan.exclude:
            problems.append(f"open spec #{n} {node.name} is in no wave and isn't excluded")
    ancestors = plan_ancestors(plan)
    for w in plan.waves:
        for n in w.issues:
            for d in nodes[n].depends if n in nodes else []:
                if d in w.issues:
                    warnings.append(f"wave {w.key}: #{n} depends on #{d} in the same wave, so they can't start together")
                elif owner.get(d) not in ancestors[w.key]:
                    problems.append(f"wave {w.key}: #{n} depends on open #{d}, which is in {owner.get(d, 'no wave')}, not an earlier wave")
    if areas is not None:
        warnings += area_warnings(plan, ancestors, areas)
    return problems, warnings


def area_warnings(plan: Plan, ancestors: dict[str, set[str]], areas: Areas) -> list[str]:
    def areas_of(w: PlanWave) -> dict[str, list[int]]:
        found: dict[str, list[int]] = {}
        for n in w.issues:
            entry = areas.specs.get(n)
            for area in entry.areas if entry else []:
                found.setdefault(area, []).append(n)
        return found

    warnings: list[str] = []
    for i, w in enumerate(plan.waves):
        mine = areas_of(w)
        for area, members in sorted(mine.items()):
            if len(members) > 1:
                warnings.append(f"wave {w.key}: {members} share area {area}")
        for other in plan.waves[i + 1:]:
            if other.key in ancestors[w.key] or w.key in ancestors[other.key]:
                continue
            shared = sorted(set(mine) & set(areas_of(other)))
            if shared:
                warnings.append(f"parallel waves {w.key} and {other.key} share areas {shared}")
    return warnings


def render_graph_table(nodes: dict[int, Node]) -> str:
    if not nodes:
        return "(no open specs)"
    w = max(len(node.name) for node in nodes.values())
    lines = [f"{'ID':<5} {'STATUS':<8} {'NAME':<{w}}  DEPENDS ON (open only)"]
    for node in nodes.values():
        deps = ", ".join(f"#{d}" for d in node.depends) or "-"
        lines.append(f"{'#' + str(node.number):<5} {node.status:<8} {node.name:<{w}}  {deps}")
    return "\n".join(lines)


def render_graph_dot(nodes: dict[int, Node]) -> str:
    out = ["digraph specs {",
           f'  graph [rankdir=LR, bgcolor="white", nodesep=0.2, ranksep=0.7, pad=0.3, fontname="{wm.FONT}"];',
           f'  node [shape=box, style="rounded,filled", fontname="{wm.FONT}", fontsize=10, height=0.3, penwidth=1];',
           '  edge [color="#475569", arrowsize=0.7];']
    for node in nodes.values():
        fill, line, text = wm.ISSUE_STYLES.get(node.status, wm.ISSUE_STYLES["missing"])
        out.append(f'  i{node.number} [label="#{node.number} {wm.dot_text(node.name)}", fillcolor="{fill}", color="{line}", fontcolor="{text}"];')
    for node in nodes.values():
        out += [f"  i{d} -> i{node.number};" for d in node.depends]
    return "\n".join([*out, "}"]) + "\n"


def resolve_parents(wave: PlanWave, numbers: dict[str, int]) -> list[int]:
    return [numbers[p] for p in wave.after]


def existing_wave_number(graph: Any, wave: PlanWave, parents: list[int]) -> int | None:
    """The wave issue with this plan wave's title, if any. One with different content means the plan and GitHub disagree."""
    title = f"{wm.TITLE_PREFIX}{wave.name}"
    for existing in graph.waves.values():
        if f"{wm.TITLE_PREFIX}{existing.name}" == title:
            if sorted(existing.issues) != sorted(wave.issues) or sorted(existing.after) != sorted(parents):
                raise ScriptFailure(f"a wave with this title has different content {kv(id=existing.number, key=wave.key)}; "
                                    f"edit it or the plan")
            return int(existing.number)
    return None


# --------------------------------------------------------------------------- io edge


def load_yaml_model(path: Path, model: Any) -> Any:
    try:
        return model.model_validate(yaml.safe_load(path.read_text(encoding="utf-8")))
    except (OSError, yaml.YAMLError, ValidationError) as e:
        raise ScriptFailure(f"cannot read {path} {kv(error=type(e).__name__, detail=str(e)[:400])}") from None


def create_wave(manager: Path, root: Path, wave: PlanWave, parents: list[int]) -> int:
    args = [sys.executable, str(manager), "--root", str(root), "create", wave.name, wave.description, "--issues", *map(str, wave.issues)]
    if parents:
        args += ["--after", *map(str, parents)]
    r = subprocess.run(args, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        raise ScriptFailure(f"wave-manager create failed {kv(key=wave.key, exit=r.returncode, stderr=r.stderr.strip()[-500:])}")
    m = wm.ISSUE_URL_RE.search(r.stdout)
    if not m:
        raise ScriptFailure(f"wave-manager printed no issue URL {kv(key=wave.key, stdout=r.stdout.strip()[:200])}")
    return int(m.group(1))


def report_validation(plan: Plan, nodes: dict[int, Node], areas: Areas | None) -> None:
    problems, warnings = validate_plan(plan, nodes, areas)
    for w in warnings:
        log.warning("%s", w)
    for p in problems:
        log.error("invalid plan %s", kv(problem=p))
    if problems:
        raise ScriptFailure(f"the plan is invalid {kv(problems=len(problems))}; fix the ERROR lines")
    log.info("plan valid %s", kv(waves=len(plan.waves), issues=sum(len(w.issues) for w in plan.waves), excluded=plan.exclude, warnings=len(warnings)))


def run_apply(root: Path, gh: Any, plan: Plan, nodes: dict[int, Node], dry: bool) -> None:
    report_validation(plan, nodes, None)
    graph = wm.load_graph(gh)
    manager = Path(__file__).resolve().parent / "wave-manager.py"
    numbers: dict[str, int] = {}
    placeholder = 0
    for wave in plan_order(plan):
        parents = resolve_parents(wave, numbers)
        existing = existing_wave_number(graph, wave, parents)
        if existing is not None:
            numbers[wave.key] = existing
            log.info("wave exists %s", kv(key=wave.key, id=existing))
        elif dry:
            placeholder -= 1
            numbers[wave.key] = placeholder
            log.info("dry run, would create %s", kv(key=wave.key, name=wave.name, issues=wave.issues, after=wave.after))
            continue
        else:
            numbers[wave.key] = create_wave(manager, root, wave, parents)
            log.info("created wave %s", kv(key=wave.key, id=numbers[wave.key], issues=wave.issues, after=parents))
            time.sleep(1.5)  # GitHub limits how fast content can be created
        print(wave.key, numbers[wave.key])


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    fmt = argparse.ArgumentDefaultsHelpFormatter
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=fmt)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--root", type=Path, default=argparse.SUPPRESS, metavar="DIR",
                        help="flat spec folder to read Depends on lines from (default: <git repo root>/specs)")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("graph", help="print open specs and their dependencies", formatter_class=fmt)
    p.add_argument("--json", action="store_true", help="print JSON")
    p.add_argument("--dot", action="store_true", help="print Graphviz")
    p = sub.add_parser("layers", help="print topological levels of the open specs", formatter_class=fmt)
    p.add_argument("--json", action="store_true", help="print JSON")
    p = sub.add_parser("propose", help="print a first-draft plan", formatter_class=fmt)
    p.add_argument("--areas", type=Path, required=True, metavar="FILE", help="YAML file of each spec's track and code areas")
    p.add_argument("--exclude", type=int, nargs="+", default=[], metavar="N", help="open specs to leave out of waves")
    p = sub.add_parser("validate", help="check a plan", formatter_class=fmt)
    p.add_argument("plan", type=Path, metavar="PLAN", help="plan YAML file")
    p.add_argument("--areas", type=Path, default=None, metavar="FILE", help="also warn about shared code areas")
    p = sub.add_parser("apply", help="create the plan's wave issues", formatter_class=fmt)
    p.add_argument("plan", type=Path, metavar="PLAN", help="plan YAML file")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    wm.setup_logging(args.verbose)
    started = time.monotonic()
    root: Path = (args.root if "root" in args else wm.default_root()).resolve()
    log.info("starting %s", kv(command=args.command, root=root))
    gh = wm.GhCli(root.parent if root.parent.is_dir() else Path.cwd())
    try:
        nodes = build_nodes(gh.list_issues(), wm.read_depends(root))
        if args.command == "graph":
            if args.dot:
                print(render_graph_dot(nodes), end="")
            elif args.json:
                print(json.dumps([n.model_dump() for n in nodes.values()], indent=2))
            else:
                print(render_graph_table(nodes))
        elif args.command == "layers":
            layers = compute_layers(nodes)
            if args.json:
                print(json.dumps(layers))
            else:
                for i, layer in enumerate(layers, start=1):
                    print(f"{i}: " + " ".join(f"#{n} {nodes[n].name}" for n in layer))
        elif args.command == "propose":
            plan = propose_plan(nodes, load_yaml_model(args.areas, Areas), args.exclude)
            print(yaml.safe_dump(plan.model_dump(), sort_keys=False, width=120), end="")
        elif args.command == "validate":
            areas = load_yaml_model(args.areas, Areas) if args.areas else None
            report_validation(load_yaml_model(args.plan, Plan), nodes, areas)
        elif args.command == "apply":
            run_apply(root, gh, load_yaml_model(args.plan, Plan), nodes, args.dry_run)
    except ScriptFailure as e:
        log.critical("%s", e)
        return 1
    log.info("done %s", kv(elapsed_s=time.monotonic() - started))
    return 0


if __name__ == "__main__":
    sys.exit(main())

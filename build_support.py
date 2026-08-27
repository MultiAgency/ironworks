"""Setuptools hooks: ship seam runtime resources, but not the repository's test modules."""
import json
import shutil
from pathlib import Path

from setuptools.command.build_py import build_py as _build_py

_ROOT = Path(__file__).resolve().parent


def _runtime_resources():
    resources = {Path("MODEL_PIN"), Path("IRONCLAW_PIN")}
    for definition_path in sorted((_ROOT / "multi/services").glob("*.json")):
        rel_definition = definition_path.relative_to(_ROOT)
        resources.add(rel_definition)
        definition = json.loads(definition_path.read_text())
        resources.update(Path(item) for item in definition["persona_parts"])
        resources.add(Path(definition["safety_tail"]))
        if definition["evaluation"]:
            evaluation = _ROOT / definition["evaluation"]
            if evaluation.is_dir():
                resources.add((evaluation / "README.md").relative_to(_ROOT))
            else:
                resources.add(evaluation.relative_to(_ROOT))
    return sorted(resources)


class BuildPy(_build_py):
    def find_package_modules(self, package, package_dir):
        modules = super().find_package_modules(package, package_dir)
        if package == "multi.seam":
            modules = [item for item in modules
                       if not item[1].startswith("test_") and item[1] != "_bridge_delivery_support"]
        return modules

    def run(self):
        shutil.rmtree(Path(self.build_lib) / "multi/seam", ignore_errors=True)
        super().run()
        bundle = Path(self.build_lib) / "multi/seam/_bundle"
        for relative in _runtime_resources():
            destination = bundle / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(_ROOT / relative, destination)

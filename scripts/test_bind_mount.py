"""M7 : une vue `mount --bind` de la source ne doit jamais être acceptée comme
destination (ni imbriquée). Linux, root requis. Tout est créé sous target/ et
démonté à la fin ; aucun dossier personnel n'est touché.
"""
import os
from pathlib import Path
import subprocess
import tempfile


def run(workspace):
    with tempfile.TemporaryDirectory(prefix="m7-bind-", dir=workspace / "target") as directory:
        folder = Path(directory)
        source = folder / "src"
        view = folder / "vue"
        (source / "inner").mkdir(parents=True)
        (source / "a.txt").write_bytes(b"a")
        view.mkdir()
        subprocess.run(["mount", "--bind", str(source), str(view)], check=True)
        try:
            env = os.environ.copy()
            env.update(SYNCENGINE_BIND_SOURCE=str(source), SYNCENGINE_BIND_VIEW=str(view))
            for command in (
                ["cargo", "test", "--locked", "-p", "syncengine-cli", "--bin", "syncengine",
                 "a_bind_view_of_the_source_is_refused", "--", "--ignored"],
                ["cargo", "test", "--locked", "-p", "sync-orchestrator", "--test", "application",
                 "a_bind_view_nested_in_the_source_is_refused", "--", "--ignored"],
            ):
                result = subprocess.run(command, cwd=workspace, env=env, timeout=900).returncode
                if result:
                    return result
            return 0
        finally:
            subprocess.run(["umount", str(view)], check=False)


if __name__ == "__main__":
    raise SystemExit(run(Path(__file__).resolve().parent.parent))

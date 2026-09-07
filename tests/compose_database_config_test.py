import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

repo_root = Path(__file__).resolve().parent.parent
environment = {key: os.environ[key] for key in ("HOME", "PATH") if key in os.environ}

with tempfile.TemporaryDirectory(prefix="aether-compose-databases-") as directory:
    fixture = Path(directory)
    for filename in (
        "docker-compose.yml",
        "docker-compose.local.yml",
        "docker-compose.single-node.yml",
        "docker-compose.release-local.yml",
    ):
        shutil.copyfile(repo_root / filename, fixture / filename)
    env_file = fixture / ".env"

    def compose_config(files, *, overrides=None):
        command = [
            "docker", "compose", "--project-name", "aether-database-fixture",
            "--project-directory", str(fixture), "--env-file", str(env_file),
            "--profile", "*",
        ]
        for filename in files:
            command.extend(["-f", str(fixture / filename)])
        return subprocess.run(
            command + ["config", "--format", "json"],
            env={**environment, **(overrides or {})},
            capture_output=True, text=True, check=False,
        )

    env_file.write_text("DB_PASSWORD=fixture-postgres\nREDIS_PASSWORD=fixture-redis\n")
    for files in (
        ["docker-compose.yml"],
        ["docker-compose.yml", "docker-compose.local.yml"],
        ["docker-compose.single-node.yml"],
    ):
        result = compose_config(files)
        assert result.returncode == 0, result.stderr
        config = json.loads(result.stdout)
        assert set(config["services"]) == {"app", "postgres", "redis"}
        assert set(config["volumes"]) == {"postgres_data"}
        app_env = config["services"]["app"]["environment"]
        assert app_env["AETHER_DATABASE_DRIVER"] == "postgres"
        assert app_env["DATABASE_URL"] == "postgresql://postgres:fixture-postgres@postgres:5432/aether"
        assert app_env["REDIS_URL"] == "redis://:fixture-redis@redis:6379/0"
        for key in ("DB_PASSWORD", "REDIS_PASSWORD"):
            result = compose_config(files, overrides={key: ""})
            assert result.returncode != 0, f"empty {key} was accepted"
            assert f"set {key} in .env" in result.stderr, result.stderr

    result = compose_config(["docker-compose.release-local.yml"])
    assert result.returncode == 0, result.stderr
    config = json.loads(result.stdout)
    assert set(config["services"]) == {"release-local-app", "postgres"}
    assert set(config["volumes"]) == {"postgres_data", "aether_release_local_root"}
    app_env = config["services"]["release-local-app"]["environment"]
    assert app_env["AETHER_DATABASE_DRIVER"] == "postgres"
    assert app_env["AETHER_DATABASE_URL"] == "postgresql://postgres:fixture-postgres@postgres:5432/aether"
    result = compose_config(["docker-compose.release-local.yml"], overrides={"DB_PASSWORD": ""})
    assert result.returncode != 0, "empty DB_PASSWORD was accepted"
    assert "set DB_PASSWORD in .env" in result.stderr, result.stderr

print("PASS: PostgreSQL/Redis Compose configurations")

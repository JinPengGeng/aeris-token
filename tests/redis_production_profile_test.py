import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

repo_root = Path(__file__).resolve().parent.parent
environment = {key: os.environ[key] for key in ("HOME", "PATH") if key in os.environ}

with tempfile.TemporaryDirectory(prefix="aether-redis-profile-") as directory:
    fixture = Path(directory)
    for filename in ("docker-compose.yml", "docker-compose.redis-production.yml"):
        shutil.copyfile(repo_root / filename, fixture / filename)
    env_file = fixture / ".env"
    env_file.write_text("DB_PASSWORD=fixture-postgres\nREDIS_PASSWORD=fixture-redis\n")

    def compose_config(files, overrides=None):
        command = [
            "docker", "compose", "--project-name", "aether-redis-profile",
            "--project-directory", str(fixture), "--env-file", str(env_file),
        ]
        for filename in files:
            command.extend(["-f", str(fixture / filename)])
        return subprocess.run(
            command + ["config", "--format", "json"],
            env={**environment, **(overrides or {})},
            capture_output=True, text=True, check=False,
        )

    base = compose_config(["docker-compose.yml"])
    assert base.returncode == 0, base.stderr
    base_config = json.loads(base.stdout)
    base_redis = base_config["services"]["redis"]
    assert "--appendonly no" in base_redis["command"]
    assert "--save \"\"" in base_redis["command"]
    assert "redis_data" not in base_config.get("volumes", {})

    production = compose_config(
        ["docker-compose.yml", "docker-compose.redis-production.yml"]
    )
    assert production.returncode == 0, production.stderr
    config = json.loads(production.stdout)
    redis = config["services"]["redis"]
    assert "--dir /data" in redis["command"]
    assert "--appendonly yes" in redis["command"]
    assert "--appendfsync everysec" in redis["command"]
    assert "--save 900 1" in redis["command"]
    assert "--save 300 10" in redis["command"]
    assert config["volumes"]["redis_data"] == {}
    assert any(volume["target"] == "/data" for volume in redis["volumes"])

    missing_password = compose_config(
        ["docker-compose.yml", "docker-compose.redis-production.yml"],
        overrides={"REDIS_PASSWORD": ""},
    )
    assert missing_password.returncode != 0, "empty REDIS_PASSWORD was accepted"
    assert "set REDIS_PASSWORD in .env" in missing_password.stderr

print("PASS: Redis production durability overlay is explicit and fail-closed")

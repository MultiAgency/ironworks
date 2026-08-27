# MultiAgency Account Service image — deps only; code + fixtures are mounted at /app.
# Pinned for reproducibility: an unpinned `pip install` rebuilds a different image over
# time. Versions match the verified-running stack (flask/psycopg) plus a current stable gunicorn.
FROM python:3.12-slim@sha256:2c941e860699f878900b0edc2403613c234d4b32eda3cc9fa7036991a2a63c4a
COPY requirements.lock /tmp/account-service-requirements.lock
RUN pip install --no-cache-dir --require-hashes -r /tmp/account-service-requirements.lock
WORKDIR /app
EXPOSE 8443
# gunicorn as the prod entry: worker timeouts + request-line/header limits that
# Flask's dev server lacks. service.py keeps app.run under __main__ as the dev entry.
# Identities hot-reload is a per-request mtime check on worker-local state — works
# per worker; 2 workers x pool max 8 stays far under pg's default 100 connections.
CMD ["sh", "-c", "exec gunicorn -w 2 --threads 4 --timeout 60 -b 0.0.0.0:${PORT:-8080} --chdir /app/deploy/account-intel/data service:app"]

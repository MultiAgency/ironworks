# MultiAgency Account Service image — deps only; code + fixtures are mounted at /app.
# Pinned for reproducibility: an unpinned `pip install` rebuilds a different image over
# time. Versions match the verified-running stack (flask/psycopg) plus a current stable gunicorn.
FROM python:3.12-slim
RUN pip install --no-cache-dir flask==3.1.3 "psycopg[binary,pool]==3.3.4" gunicorn==23.0.0
WORKDIR /app
EXPOSE 8443
# gunicorn as the prod entry: worker timeouts + request-line/header limits that
# Flask's dev server lacks. service.py keeps app.run under __main__ as the dev entry.
# Identities hot-reload is a per-request mtime check on worker-local state — works
# per worker; 2 workers x pool max 8 stays far under pg's default 100 connections.
CMD ["sh", "-c", "exec gunicorn -w 2 --threads 4 --timeout 60 -b 0.0.0.0:${PORT:-8080} --chdir /app/deploy/account-intel/data service:app"]

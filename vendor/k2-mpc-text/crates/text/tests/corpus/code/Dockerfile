# syntax=docker/dockerfile:1
FROM python:3.12-slim AS base
WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt \
    && useradd --create-home app

COPY . .
USER app
EXPOSE 8000
CMD ["python", "-m", "server", "--port", "8000"]

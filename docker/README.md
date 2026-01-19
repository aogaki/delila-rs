# DELILA Docker Services

## MongoDB + Mongo Express

Run history and configuration storage.

### Quick Start

```bash
cd docker
docker compose up -d
```

### Services

| Service | Port | Description |
|---------|------|-------------|
| MongoDB | 27017 | Database |
| Mongo Express | 8082 | Web UI for MongoDB |

### Access

- **Mongo Express**: http://localhost:8082
- **MongoDB**: `mongodb://delila:delila_pass@localhost:27017/delila`

### Credentials

| Field | Value |
|-------|-------|
| Username | `delila` |
| Password | `delila_pass` |
| Database | `delila` |

### Data Persistence

Data is stored in a Docker volume `mongo_data`. To reset:

```bash
docker compose down -v
docker compose up -d
```

### Collections

- `runs`: Run history with statistics, config snapshots, and error logs

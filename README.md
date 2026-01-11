# Catbus

A WebTransport-based message bus for real-time state synchronization between browsers, backend services, and workers.

## Features

- **WebTransport over HTTP/3** - Low latency, multiplexed connections
- **Channel-based pub/sub** - Hierarchical channels with wildcard subscriptions
- **Fine-grained permissions** - Read/write/create grants per channel pattern
- **Two token types**:
  - `sub-` tokens: Stateless, signed tokens with embedded grants
  - `role-` tokens: Stateful, revocable tokens with grants stored in Postgres
- **PostgreSQL backend** - LISTEN/NOTIFY for cross-instance fan-out
- **Ring buffer** - Catch-up on reconnect for missed messages

## Quick Start

### 1. Create a token

```bash
# Set your signing secret
export CATBUS_SECRET=your-secret-key

# Create a token with full access to project channels
catbus grant -p all 'project.*'

# Output:
# sub-eyJjbGllbnRfaWQiOi...
# Client ID: 6f8f5d3f-f3da-434b-8b12-6fe1ec21ba4e
# Grants:
#   read:project.*
#   write:project.*
#   create:project.*
```

### 2. Start the server

```bash
# Initialize the database
export DATABASE_URL=postgres://user:pass@localhost/catbus
catbus init

# Start the daemon (foreground by default)
catbusd --cert cert.pem --key key.pem

# Or run as a background daemon
catbusd -d --pidfile /var/run/catbusd.pid
```

### 3. Connect from your app

```typescript
import { CatbusClient } from '@catbus/client';

const client = new CatbusClient({
  url: 'https://localhost:4433',
  token: 'sub-eyJjbGllbnRfaWQiOi...'
});

await client.connect();

// Subscribe to messages
await client.subscribe('project.123.*', (channel, payload) => {
  console.log(`${channel}:`, payload);
});

// Publish a message
await client.publish('project.123.updates', { action: 'save', data: {...} });
```

## Binaries

Catbus ships as two binaries:

| Binary | Purpose |
|--------|---------|
| `catbusd` | The server daemon |
| `catbus` | CLI for token/role management |

## catbusd

The WebTransport server daemon.

```bash
catbusd [OPTIONS]
```

| Option | Env | Description |
|--------|-----|-------------|
| `-b, --bind` | `CATBUS_BIND` | Address to bind (default: `0.0.0.0:4433`) |
| `--cert` | `CATBUS_CERT` | Path to TLS certificate (PEM) |
| `--key` | `CATBUS_KEY` | Path to TLS private key (PEM) |
| `--secret` | `CATBUS_SECRET` | Token signing secret |
| `--database-url` | `DATABASE_URL` | PostgreSQL connection URL |
| `--admin-key` | `CATBUS_ADMIN_KEY` | Admin key for full access |
| `-d, --daemon` | | Run as background daemon |
| `--pidfile` | `CATBUS_PIDFILE` | PID file path (with `-d`) |
| `--log-level` | `RUST_LOG` | Log level (default: `info`) |

### Examples

```bash
# Foreground (for Docker/systemd)
catbusd --cert cert.pem --key key.pem

# Background daemon with pidfile
catbusd -d --pidfile /var/run/catbusd.pid

# With all options via environment
export DATABASE_URL=postgres://localhost/catbus
export CATBUS_SECRET=my-secret
export CATBUS_CERT=/etc/catbus/cert.pem
export CATBUS_KEY=/etc/catbus/key.pem

catbusd
```

## catbus

CLI tool for token and role management.

### `catbus grant`

Create tokens or add grants to existing roles.

```bash
# Create a new sub token
catbus grant -p read -p write 'project.*' 'events.*'

# With a specific client ID
catbus grant -p all 'user.alice.*' --client-id alice

# Add grants to an existing role
catbus grant -p read 'audit.*' --to role-abc123
```

### `catbus role`

Manage stateful role tokens.

```bash
# Create a new role
catbus role create --name "Editor"

# Show role grants
catbus role show role-abc123

# Delete a role (invalidates immediately)
catbus role delete role-abc123
```

### `catbus revoke`

Remove grants from a role.

```bash
catbus revoke -p write 'project.*' --from role-abc123
```

### `catbus init`

Initialize the database schema.

```bash
catbus init --database-url postgres://localhost/catbus
```

### `catbus status`

Check database connectivity.

```bash
catbus status
```

## Channel Patterns

Channels are dot-separated hierarchical names:

```
project.abc123.updates
user.alice.inbox
events.public.announcements
```

Patterns support trailing wildcards for subscriptions and grants:

```
project.*           # All project channels
project.abc123.*    # All channels under project.abc123
user.alice.*        # Alice's channels
*                   # Everything (admin only)
```

Wildcards are only allowed at the end for O(1) matching performance.

## Permission Types

| Permission | Description |
|------------|-------------|
| `read` | Subscribe to matching channels |
| `write` | Publish to matching channels |
| `create` | Create new channels (future) |
| `all` | All of the above |

## Docker

```bash
# Build
docker build -t catbus .

# Run with docker-compose
docker-compose up
```

## Client Libraries

- **TypeScript/JavaScript**: [`@catbus/client`](./client-node/) - Full client with React hooks

## License

MIT

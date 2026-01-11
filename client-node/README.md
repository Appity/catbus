# @catbus/client

TypeScript client for [Catbus](https://github.com/your-org/catbus) WebTransport message bus.

## Installation

```bash
npm install @catbus/client
```

## Quick Start

```typescript
import { CatbusClient } from '@catbus/client';

const client = new CatbusClient({
  url: 'https://your-catbus-server:4433',
  token: 'sub-eyJjbGllbnRfaWQiOi...'
});

await client.connect();

// Subscribe to a channel pattern
const subscription = await client.subscribe('project.123.*', (channel, payload) => {
  console.log(`Received on ${channel}:`, payload);
});

// Publish a message
await client.publish('project.123.updates', {
  action: 'save',
  timestamp: Date.now()
});

// Unsubscribe when done
await subscription.unsubscribe();

// Disconnect
await client.disconnect();
```

## API Reference

### `CatbusClient`

The main client class for connecting to a Catbus server.

#### Constructor

```typescript
new CatbusClient(config: CatbusConfig)
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | `string` | required | Server URL (e.g., `https://localhost:4433`) |
| `token` | `string` | required | Authentication token |
| `autoReconnect` | `boolean` | `true` | Auto-reconnect on disconnect |
| `reconnectDelay` | `number` | `1000` | Initial reconnect delay (ms) |
| `maxReconnectDelay` | `number` | `30000` | Maximum reconnect delay (ms) |
| `pingInterval` | `number` | `30000` | Keepalive ping interval (ms) |

#### Methods

##### `connect(): Promise<void>`

Connect to the server and authenticate.

```typescript
await client.connect();
```

##### `disconnect(): Promise<void>`

Disconnect from the server. Disables auto-reconnect.

```typescript
await client.disconnect();
```

##### `subscribe<T>(pattern: string, handler: MessageHandler<T>): Promise<Subscription>`

Subscribe to messages matching a channel pattern.

```typescript
const sub = await client.subscribe('events.*', (channel, payload) => {
  console.log(channel, payload);
});

// Later: unsubscribe
await sub.unsubscribe();
```

##### `publish(channel: string, payload: unknown): Promise<void>`

Publish a message to a channel.

```typescript
await client.publish('events.user-joined', { userId: '123', name: 'Alice' });
```

##### `ping(): Promise<number>`

Send a ping and return round-trip time in milliseconds.

```typescript
const latency = await client.ping();
console.log(`Latency: ${latency}ms`);
```

##### `onStateChange(handler: StateChangeHandler): () => void`

Register a handler for connection state changes. Returns an unsubscribe function.

```typescript
const unsubscribe = client.onStateChange((state, error) => {
  console.log('Connection state:', state);
  if (error) console.error('Error:', error);
});
```

#### Properties

- `connectionState: ConnectionState` - Current state: `'disconnected'`, `'connecting'`, `'authenticating'`, `'connected'`, or `'reconnecting'`
- `id: string | null` - Client ID from authentication (null for admin/role tokens)

## React Hooks

Import React hooks from `@catbus/client/react` or directly from the main package.

### `useCatbus`

Create and manage a Catbus connection.

```tsx
import { useCatbus } from '@catbus/client/react';

function App() {
  const { client, state, error } = useCatbus({
    url: 'https://localhost:4433',
    token: process.env.NEXT_PUBLIC_CATBUS_TOKEN!
  });

  if (state === 'connecting') return <div>Connecting...</div>;
  if (error) return <div>Error: {error.message}</div>;
  if (state !== 'connected') return <div>Disconnected</div>;

  return <Dashboard client={client!} />;
}
```

### `useSubscription`

Subscribe to messages and collect them in an array.

```tsx
import { useSubscription } from '@catbus/client/react';

function ActivityFeed({ client, projectId }) {
  const messages = useSubscription(client, `project.${projectId}.*`, {
    maxMessages: 50,  // Keep last 50 messages
    onMessage: (channel, payload) => {
      // Optional: handle each message as it arrives
      console.log('New message:', channel, payload);
    }
  });

  return (
    <ul>
      {messages.map((msg, i) => (
        <li key={i}>
          <strong>{msg.channel}</strong>: {JSON.stringify(msg.payload)}
        </li>
      ))}
    </ul>
  );
}
```

### `useLatestMessage`

Get only the most recent message (useful for state sync).

```tsx
import { useLatestMessage } from '@catbus/client/react';

function DocumentStatus({ client, docId }) {
  const latest = useLatestMessage(client, `doc.${docId}.status`);

  if (!latest) return <div>Loading...</div>;

  return (
    <div>
      Status: {latest.payload.status}
      Last updated: {new Date(latest.payload.timestamp).toLocaleString()}
    </div>
  );
}
```

### `usePublish`

Get a stable publish function with loading/error state.

```tsx
import { usePublish } from '@catbus/client/react';

function SendNotification({ client }) {
  const { publish, isPending, error } = usePublish(client);
  const [message, setMessage] = useState('');

  const handleSend = async () => {
    try {
      await publish('notifications.all', {
        text: message,
        timestamp: Date.now()
      });
      setMessage('');
    } catch (e) {
      // Error is also available via `error`
    }
  };

  return (
    <div>
      <input
        value={message}
        onChange={e => setMessage(e.target.value)}
        disabled={isPending}
      />
      <button onClick={handleSend} disabled={isPending || !message}>
        {isPending ? 'Sending...' : 'Send'}
      </button>
      {error && <div className="error">{error.message}</div>}
    </div>
  );
}
```

### `useConnectionState`

Efficiently subscribe to connection state changes.

```tsx
import { useConnectionState } from '@catbus/client/react';

function ConnectionIndicator({ client }) {
  const state = useConnectionState(client);

  const colors = {
    connected: 'green',
    connecting: 'yellow',
    authenticating: 'yellow',
    reconnecting: 'orange',
    disconnected: 'red'
  };

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <div style={{
        width: 8,
        height: 8,
        borderRadius: '50%',
        backgroundColor: colors[state]
      }} />
      {state}
    </div>
  );
}
```

## Full React Example

```tsx
import { useCatbus, useSubscription, usePublish } from '@catbus/client/react';

function ChatRoom({ roomId }) {
  const { client, state } = useCatbus({
    url: process.env.NEXT_PUBLIC_CATBUS_URL!,
    token: process.env.NEXT_PUBLIC_CATBUS_TOKEN!
  });

  const messages = useSubscription(client, `chat.${roomId}.*`, {
    maxMessages: 100
  });

  const { publish, isPending } = usePublish(client);
  const [input, setInput] = useState('');

  const sendMessage = async () => {
    if (!input.trim()) return;
    await publish(`chat.${roomId}.messages`, {
      text: input,
      sender: 'me',
      timestamp: Date.now()
    });
    setInput('');
  };

  if (state !== 'connected') {
    return <div>Connecting to chat...</div>;
  }

  return (
    <div>
      <div className="messages">
        {messages.map((msg, i) => (
          <div key={i} className="message">
            <strong>{msg.payload.sender}:</strong> {msg.payload.text}
          </div>
        ))}
      </div>
      <div className="input">
        <input
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => e.key === 'Enter' && sendMessage()}
          placeholder="Type a message..."
        />
        <button onClick={sendMessage} disabled={isPending}>
          Send
        </button>
      </div>
    </div>
  );
}
```

## NextJS Integration

### App Router (Server Components)

Create a client component wrapper:

```tsx
// components/CatbusProvider.tsx
'use client';

import { createContext, useContext, ReactNode } from 'react';
import { useCatbus } from '@catbus/client/react';
import type { CatbusClient } from '@catbus/client';

const CatbusContext = createContext<CatbusClient | null>(null);

export function CatbusProvider({ children, token }: { children: ReactNode; token: string }) {
  const { client, state } = useCatbus({
    url: process.env.NEXT_PUBLIC_CATBUS_URL!,
    token
  });

  if (state !== 'connected') {
    return <div>Connecting...</div>;
  }

  return (
    <CatbusContext.Provider value={client}>
      {children}
    </CatbusContext.Provider>
  );
}

export function useCatbusClient() {
  const client = useContext(CatbusContext);
  if (!client) throw new Error('useCatbusClient must be used within CatbusProvider');
  return client;
}
```

```tsx
// app/layout.tsx
import { CatbusProvider } from '@/components/CatbusProvider';

export default function RootLayout({ children }) {
  // Get token from your auth system
  const token = getTokenForUser();

  return (
    <html>
      <body>
        <CatbusProvider token={token}>
          {children}
        </CatbusProvider>
      </body>
    </html>
  );
}
```

## Yjs Integration (Collaborative Editing)

Catbus has first-class support for [Yjs](https://yjs.dev) CRDT synchronization, enabling real-time collaborative editing across multiple users and AI agents.

### Yjs-Specific Message Types

Instead of generic `publish()`, use the dedicated Yjs methods:

```typescript
// Send a Yjs document update
client.yjsUpdate(channel: string, update: Uint8Array, origin?: string): Promise<void>

// Send awareness update (cursors, presence)
client.yjsAwareness(channel: string, update: Uint8Array): Promise<void>

// Request sync from other clients
client.yjsSyncRequest(channel: string, stateVector: Uint8Array): Promise<void>
```

### Basic Yjs Integration

```typescript
import * as Y from 'yjs';
import { CatbusClient } from '@catbus/client';

const ydoc = new Y.Doc();
const client = new CatbusClient({ url, token });

await client.connect();
await client.subscribe(`doc.${docId}.*`);

// Send local changes
ydoc.on('update', (update: Uint8Array, origin: any) => {
  // Don't broadcast updates that came from remote
  if (origin !== 'remote') {
    client.yjsUpdate(`doc.${docId}`, update, client.id);
  }
});

// Receive remote changes
client.onYjsUpdate((channel, update, origin) => {
  // Skip our own updates (echo suppression)
  if (origin !== client.id) {
    Y.applyUpdate(ydoc, update, 'remote');
  }
});
```

### React Hook for Yjs

```tsx
import { useYjsSync } from '@catbus/client/react';
import { useEditor } from '@tiptap/react';

function CollaborativeEditor({ docId }) {
  const { client, state } = useCatbus({ url, token });

  const { ydoc, synced, peers } = useYjsSync(client, `doc.${docId}`, {
    // Enable awareness for cursor sync
    awareness: true,

    // Called when initial sync completes
    onSynced: () => console.log('Document synced'),

    // Called when peers change
    onPeersChange: (peers) => console.log('Active peers:', peers.length),
  });

  const editor = useEditor({
    extensions: [
      // ... your extensions
      Collaboration.configure({ document: ydoc }),
      CollaborationCursor.configure({
        provider: ydoc, // Catbus handles the transport
      }),
    ],
  });

  if (!synced) return <div>Syncing document...</div>;

  return <EditorContent editor={editor} />;
}
```

### Multi-Server Yjs Topology

For NextJS deployments with multiple server instances, Catbus handles fan-out automatically:

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Browser 1  │     │  Browser 2  │     │  AI Agent   │
│  (User A)   │     │  (User B)   │     │  (Claude)   │
└──────┬──────┘     └──────┬──────┘     └──────┬──────┘
       │                   │                   │
       │    WebTransport   │    WebTransport   │    HTTP/WS
       │                   │                   │
       ▼                   ▼                   ▼
┌─────────────────────────────────────────────────────┐
│                    Catbus Server                     │
│  ┌─────────────────────────────────────────────┐    │
│  │     Channel: doc.123                         │    │
│  │     Ring Buffer (catch-up on reconnect)     │    │
│  └─────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

Each Yjs update is:
1. Persisted to the ring buffer (for catch-up)
2. Broadcast to all subscribers on that document channel
3. Delivered with origin tracking (for echo suppression)

### Agent Integration

AI agents can participate in collaborative editing:

```python
# Python agent using Catbus
import catbus
from yjs import YDoc, encode_state_as_update

doc = YDoc()
client = catbus.connect(url, token)

# Subscribe to document
client.subscribe(f"doc.{doc_id}.*")

# Apply remote updates
@client.on_yjs_update
def handle_update(channel, update, origin):
    doc.apply_update(update)

# Make edits
with doc.begin_transaction() as txn:
    text = doc.get_text("content")
    text.insert(0, "AI suggestion: ")

update = encode_state_as_update(doc)
client.yjs_update(f"doc.{doc_id}", update, origin="agent")
```

### Catching Up After Disconnect

When a client reconnects, it should request missed updates:

```typescript
client.onReconnect(async () => {
  // Get state vector of what we have
  const sv = Y.encodeStateVector(ydoc);

  // Request updates we're missing
  await client.yjsSyncRequest(`doc.${docId}`, sv);
});

client.onYjsSyncResponse((channel, update) => {
  // Apply the diff
  Y.applyUpdate(ydoc, update, 'sync');
});
```

## Browser Support

Catbus uses WebTransport which requires:
- Chrome/Edge 97+
- Firefox 114+ (behind flag)
- Safari: Not yet supported

For broader support, consider using a WebSocket fallback (not included in this package).

## License

MIT

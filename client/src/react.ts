import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { CatbusClient } from "./client.js";
import type {
  CatbusConfig,
  ConnectionState,
  MessageHandler,
} from "./types.js";

/**
 * React hook for Catbus client connection
 *
 * Creates and manages a CatbusClient instance, handling connection
 * lifecycle and cleanup automatically.
 *
 * @example
 * ```tsx
 * function App() {
 *   const { client, state, error } = useCatbus({
 *     url: "https://localhost:4433",
 *     token: "sub-abc123..."
 *   });
 *
 *   if (state === "connecting") return <div>Connecting...</div>;
 *   if (error) return <div>Error: {error.message}</div>;
 *
 *   return <MyComponent client={client} />;
 * }
 * ```
 */
export function useCatbus(config: CatbusConfig): {
  client: CatbusClient | null;
  state: ConnectionState;
  error: Error | null;
} {
  const [client, setClient] = useState<CatbusClient | null>(null);
  const [state, setState] = useState<ConnectionState>("disconnected");
  const [error, setError] = useState<Error | null>(null);
  const configRef = useRef(config);

  // Update config ref without recreating client
  useEffect(() => {
    configRef.current = config;
  }, [config]);

  useEffect(() => {
    const c = new CatbusClient(configRef.current);
    setClient(c);

    const unsubscribe = c.onStateChange((newState, err) => {
      setState(newState);
      if (err) setError(err);
    });

    c.connect().catch((err) => {
      setError(err);
    });

    return () => {
      unsubscribe();
      c.disconnect();
    };
  }, [config.url, config.token]);

  return { client, state, error };
}

/**
 * React hook for subscribing to Catbus messages
 *
 * Automatically subscribes when the component mounts and unsubscribes
 * when it unmounts.
 *
 * @example
 * ```tsx
 * function ProjectUpdates({ client, projectId }) {
 *   const messages = useSubscription(client, `project.${projectId}.*`);
 *
 *   return (
 *     <ul>
 *       {messages.map((msg, i) => (
 *         <li key={i}>{msg.channel}: {JSON.stringify(msg.payload)}</li>
 *       ))}
 *     </ul>
 *   );
 * }
 * ```
 */
export function useSubscription<T = unknown>(
  client: CatbusClient | null,
  pattern: string,
  options?: {
    /** Maximum messages to keep in buffer (default: 100) */
    maxMessages?: number;
    /** Handler called for each message (optional) */
    onMessage?: MessageHandler<T>;
  }
): Array<{ channel: string; payload: T }> {
  const maxMessages = options?.maxMessages ?? 100;
  const [messages, setMessages] = useState<Array<{ channel: string; payload: T }>>([]);
  const onMessageRef = useRef(options?.onMessage);

  useEffect(() => {
    onMessageRef.current = options?.onMessage;
  }, [options?.onMessage]);

  useEffect(() => {
    if (!client || client.connectionState !== "connected") {
      return;
    }

    let subscription: { unsubscribe(): Promise<void> } | null = null;

    const handler: MessageHandler<T> = (channel, payload) => {
      setMessages((prev) => {
        const next = [...prev, { channel, payload }];
        if (next.length > maxMessages) {
          return next.slice(-maxMessages);
        }
        return next;
      });

      onMessageRef.current?.(channel, payload);
    };

    client.subscribe<T>(pattern, handler).then((sub) => {
      subscription = sub;
    });

    return () => {
      subscription?.unsubscribe();
    };
  }, [client, pattern, maxMessages]);

  return messages;
}

/**
 * React hook for the latest message on a subscription
 *
 * Only keeps track of the most recent message, useful for state sync.
 *
 * @example
 * ```tsx
 * function CurrentState({ client, docId }) {
 *   const latest = useLatestMessage(client, `doc.${docId}.state`);
 *
 *   if (!latest) return <div>Loading...</div>;
 *   return <pre>{JSON.stringify(latest.payload, null, 2)}</pre>;
 * }
 * ```
 */
export function useLatestMessage<T = unknown>(
  client: CatbusClient | null,
  pattern: string
): { channel: string; payload: T } | null {
  const [latest, setLatest] = useState<{ channel: string; payload: T } | null>(null);

  useEffect(() => {
    if (!client || client.connectionState !== "connected") {
      return;
    }

    let subscription: { unsubscribe(): Promise<void> } | null = null;

    const handler: MessageHandler<T> = (channel, payload) => {
      setLatest({ channel, payload });
    };

    client.subscribe<T>(pattern, handler).then((sub) => {
      subscription = sub;
    });

    return () => {
      subscription?.unsubscribe();
    };
  }, [client, pattern]);

  return latest;
}

/**
 * React hook for publishing messages
 *
 * Returns a stable publish function that can be used in event handlers.
 *
 * @example
 * ```tsx
 * function SendButton({ client }) {
 *   const { publish, isPending, error } = usePublish(client);
 *
 *   const handleClick = () => {
 *     publish("notifications.all", { text: "Hello!" });
 *   };
 *
 *   return (
 *     <button onClick={handleClick} disabled={isPending}>
 *       {isPending ? "Sending..." : "Send"}
 *     </button>
 *   );
 * }
 * ```
 */
export function usePublish(client: CatbusClient | null): {
  publish: (channel: string, payload: unknown) => Promise<void>;
  isPending: boolean;
  error: Error | null;
} {
  const [isPending, setIsPending] = useState(false);
  const [error, setError] = useState<Error | null>(null);

  const publish = useCallback(
    async (channel: string, payload: unknown) => {
      if (!client) {
        throw new Error("Client not connected");
      }

      setIsPending(true);
      setError(null);

      try {
        await client.publish(channel, payload);
      } catch (e) {
        setError(e as Error);
        throw e;
      } finally {
        setIsPending(false);
      }
    },
    [client]
  );

  return { publish, isPending, error };
}

/**
 * React hook for connection state
 *
 * Uses useSyncExternalStore for optimal performance.
 *
 * @example
 * ```tsx
 * function ConnectionStatus({ client }) {
 *   const state = useConnectionState(client);
 *
 *   return <div>Status: {state}</div>;
 * }
 * ```
 */
export function useConnectionState(client: CatbusClient | null): ConnectionState {
  const subscribe = useCallback(
    (onStoreChange: () => void) => {
      if (!client) return () => {};
      return client.onStateChange(onStoreChange);
    },
    [client]
  );

  const getSnapshot = useCallback(() => {
    return client?.connectionState ?? "disconnected";
  }, [client]);

  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

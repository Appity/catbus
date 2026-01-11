/**
 * Messages sent from client to server
 */
export type ClientMessage =
  | { type: "auth"; token: string }
  | { type: "subscribe"; pattern: string }
  | { type: "unsubscribe"; pattern: string }
  | { type: "publish"; channel: string; payload: unknown }
  | { type: "ping"; seq: number };

/**
 * Messages sent from server to client
 */
export type ServerMessage =
  | { type: "auth_ok"; client_id: string | null }
  | { type: "auth_error"; message: string }
  | { type: "subscribed"; pattern: string }
  | { type: "subscribe_error"; pattern: string; message: string }
  | { type: "unsubscribed"; pattern: string }
  | { type: "published"; channel: string }
  | { type: "publish_error"; channel: string; message: string }
  | { type: "message"; channel: string; payload: unknown }
  | { type: "pong"; seq: number }
  | { type: "error"; message: string };

/**
 * Handler for incoming messages on subscribed channels
 */
export type MessageHandler<T = unknown> = (
  channel: string,
  payload: T
) => void | Promise<void>;

/**
 * Connection state
 */
export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "authenticating"
  | "connected"
  | "reconnecting";

/**
 * Connection state change handler
 */
export type StateChangeHandler = (
  state: ConnectionState,
  error?: Error
) => void;

/**
 * Catbus client configuration
 */
export interface CatbusConfig {
  /** Server URL (e.g., "https://localhost:4433") */
  url: string;
  /** Authentication token (sub-*, role-*, or admin key) */
  token: string;
  /** Auto-reconnect on disconnect (default: true) */
  autoReconnect?: boolean;
  /** Reconnect delay in ms (default: 1000) */
  reconnectDelay?: number;
  /** Maximum reconnect delay in ms (default: 30000) */
  maxReconnectDelay?: number;
  /** Ping interval in ms (default: 30000) */
  pingInterval?: number;
}

/**
 * Subscription options
 */
export interface SubscribeOptions {
  /** Catch-up from sequence number (for reconnect) */
  since?: number;
}

/**
 * Subscription handle for unsubscribing
 */
export interface Subscription {
  /** The subscribed pattern */
  pattern: string;
  /** Unsubscribe from this pattern */
  unsubscribe(): Promise<void>;
}

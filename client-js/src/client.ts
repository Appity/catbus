import type {
  CatbusConfig,
  ClientMessage,
  ConnectionState,
  MessageHandler,
  ServerMessage,
  StateChangeHandler,
  Subscription,
} from "./types.js";

/**
 * Catbus WebTransport client
 *
 * Connects to a Catbus server over WebTransport for real-time pub/sub messaging.
 *
 * @example
 * ```typescript
 * const client = new CatbusClient({
 *   url: "https://localhost:4433",
 *   token: "sub-abc123..."
 * });
 *
 * await client.connect();
 *
 * // Subscribe to messages
 * const sub = await client.subscribe("project.123.*", (channel, payload) => {
 *   console.log(`Received on ${channel}:`, payload);
 * });
 *
 * // Publish a message
 * await client.publish("project.123.updates", { action: "save" });
 *
 * // Clean up
 * await sub.unsubscribe();
 * await client.disconnect();
 * ```
 */
export class CatbusClient {
  private config: Required<CatbusConfig>;
  private transport: WebTransport | null = null;
  private stream: {
    readable: ReadableStreamDefaultReader<Uint8Array>;
    writable: WritableStreamDefaultWriter<Uint8Array>;
  } | null = null;

  private state: ConnectionState = "disconnected";
  private clientId: string | null = null;
  private pingSeq = 0;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectAttempts = 0;

  private subscriptions = new Map<string, Set<MessageHandler>>();
  private pendingSubscribes = new Map<
    string,
    { resolve: () => void; reject: (e: Error) => void }
  >();
  private pendingUnsubscribes = new Map<
    string,
    { resolve: () => void; reject: (e: Error) => void }
  >();
  private pendingPublishes = new Map<
    string,
    { resolve: () => void; reject: (e: Error) => void }
  >();
  private pendingPings = new Map<
    number,
    { resolve: () => void; reject: (e: Error) => void }
  >();

  private stateHandlers = new Set<StateChangeHandler>();
  private encoder = new TextEncoder();
  private decoder = new TextDecoder();

  constructor(config: CatbusConfig) {
    this.config = {
      autoReconnect: true,
      reconnectDelay: 1000,
      maxReconnectDelay: 30000,
      pingInterval: 30000,
      ...config,
    };
  }

  /**
   * Current connection state
   */
  get connectionState(): ConnectionState {
    return this.state;
  }

  /**
   * Client ID from authentication (null for admin/role tokens)
   */
  get id(): string | null {
    return this.clientId;
  }

  /**
   * Register a handler for connection state changes
   */
  onStateChange(handler: StateChangeHandler): () => void {
    this.stateHandlers.add(handler);
    return () => this.stateHandlers.delete(handler);
  }

  /**
   * Connect to the Catbus server
   */
  async connect(): Promise<void> {
    if (this.state !== "disconnected" && this.state !== "reconnecting") {
      throw new Error(`Cannot connect in state: ${this.state}`);
    }

    this.setState("connecting");

    try {
      // Create WebTransport connection
      this.transport = new WebTransport(this.config.url);
      await this.transport.ready;

      // Open bidirectional stream for messaging
      const bidiStream = await this.transport.createBidirectionalStream();
      this.stream = {
        readable: bidiStream.readable.getReader(),
        writable: bidiStream.writable.getWriter(),
      };

      // Authenticate
      this.setState("authenticating");
      await this.send({ type: "auth", token: this.config.token });

      // Wait for auth response
      const response = await this.readMessage();
      if (response.type === "auth_ok") {
        this.clientId = response.client_id;
        this.setState("connected");
        this.reconnectAttempts = 0;

        // Start ping timer
        this.startPingTimer();

        // Start message receive loop
        this.receiveLoop();

        // Resubscribe to existing subscriptions
        await this.resubscribe();
      } else if (response.type === "auth_error") {
        throw new Error(`Authentication failed: ${response.message}`);
      } else {
        throw new Error(`Unexpected response: ${response.type}`);
      }
    } catch (error) {
      this.cleanup();
      this.setState("disconnected", error as Error);
      throw error;
    }
  }

  /**
   * Disconnect from the server
   */
  async disconnect(): Promise<void> {
    this.config.autoReconnect = false;
    this.cleanup();
    this.setState("disconnected");
  }

  /**
   * Subscribe to a channel pattern
   */
  async subscribe<T = unknown>(
    pattern: string,
    handler: MessageHandler<T>
  ): Promise<Subscription> {
    // Add handler to local subscriptions
    let handlers = this.subscriptions.get(pattern);
    if (!handlers) {
      handlers = new Set();
      this.subscriptions.set(pattern, handlers);
    }
    handlers.add(handler as MessageHandler);

    // Send subscribe if connected and first handler
    if (this.state === "connected" && handlers.size === 1) {
      await this.doSubscribe(pattern);
    }

    return {
      pattern,
      unsubscribe: async () => {
        const h = this.subscriptions.get(pattern);
        if (h) {
          h.delete(handler as MessageHandler);
          if (h.size === 0) {
            this.subscriptions.delete(pattern);
            if (this.state === "connected") {
              await this.doUnsubscribe(pattern);
            }
          }
        }
      },
    };
  }

  /**
   * Publish a message to a channel
   */
  async publish(channel: string, payload: unknown): Promise<void> {
    if (this.state !== "connected") {
      throw new Error("Not connected");
    }

    await this.send({ type: "publish", channel, payload });

    // Wait for confirmation
    return new Promise((resolve, reject) => {
      this.pendingPublishes.set(channel, { resolve, reject });

      // Timeout after 10 seconds
      setTimeout(() => {
        if (this.pendingPublishes.has(channel)) {
          this.pendingPublishes.delete(channel);
          reject(new Error("Publish timeout"));
        }
      }, 10000);
    });
  }

  /**
   * Send a ping and wait for pong
   */
  async ping(): Promise<number> {
    if (this.state !== "connected") {
      throw new Error("Not connected");
    }

    const seq = ++this.pingSeq;
    const start = Date.now();

    await this.send({ type: "ping", seq });

    return new Promise((resolve, reject) => {
      this.pendingPings.set(seq, {
        resolve: () => resolve(Date.now() - start),
        reject,
      });

      setTimeout(() => {
        if (this.pendingPings.has(seq)) {
          this.pendingPings.delete(seq);
          reject(new Error("Ping timeout"));
        }
      }, 5000);
    });
  }

  private async doSubscribe(pattern: string): Promise<void> {
    await this.send({ type: "subscribe", pattern });

    return new Promise((resolve, reject) => {
      this.pendingSubscribes.set(pattern, { resolve, reject });

      setTimeout(() => {
        if (this.pendingSubscribes.has(pattern)) {
          this.pendingSubscribes.delete(pattern);
          reject(new Error("Subscribe timeout"));
        }
      }, 10000);
    });
  }

  private async doUnsubscribe(pattern: string): Promise<void> {
    await this.send({ type: "unsubscribe", pattern });

    return new Promise((resolve, reject) => {
      this.pendingUnsubscribes.set(pattern, { resolve, reject });

      setTimeout(() => {
        if (this.pendingUnsubscribes.has(pattern)) {
          this.pendingUnsubscribes.delete(pattern);
          reject(new Error("Unsubscribe timeout"));
        }
      }, 10000);
    });
  }

  private async resubscribe(): Promise<void> {
    for (const pattern of this.subscriptions.keys()) {
      try {
        await this.doSubscribe(pattern);
      } catch (e) {
        console.error(`Failed to resubscribe to ${pattern}:`, e);
      }
    }
  }

  private async send(msg: ClientMessage): Promise<void> {
    if (!this.stream) {
      throw new Error("Not connected");
    }

    const data = this.encoder.encode(JSON.stringify(msg));
    await this.stream.writable.write(data);
  }

  private async readMessage(): Promise<ServerMessage> {
    if (!this.stream) {
      throw new Error("Not connected");
    }

    const result = await this.stream.readable.read();
    if (result.done) {
      throw new Error("Connection closed");
    }

    return JSON.parse(this.decoder.decode(result.value)) as ServerMessage;
  }

  private async receiveLoop(): Promise<void> {
    try {
      while (this.state === "connected" && this.stream) {
        const msg = await this.readMessage();
        this.handleMessage(msg);
      }
    } catch (e) {
      if (this.state === "connected") {
        console.error("Receive error:", e);
        this.handleDisconnect();
      }
    }
  }

  private handleMessage(msg: ServerMessage): void {
    switch (msg.type) {
      case "subscribed": {
        const pending = this.pendingSubscribes.get(msg.pattern);
        if (pending) {
          this.pendingSubscribes.delete(msg.pattern);
          pending.resolve();
        }
        break;
      }

      case "subscribe_error": {
        const pending = this.pendingSubscribes.get(msg.pattern);
        if (pending) {
          this.pendingSubscribes.delete(msg.pattern);
          pending.reject(new Error(msg.message));
        }
        break;
      }

      case "unsubscribed": {
        const pending = this.pendingUnsubscribes.get(msg.pattern);
        if (pending) {
          this.pendingUnsubscribes.delete(msg.pattern);
          pending.resolve();
        }
        break;
      }

      case "published": {
        const pending = this.pendingPublishes.get(msg.channel);
        if (pending) {
          this.pendingPublishes.delete(msg.channel);
          pending.resolve();
        }
        break;
      }

      case "publish_error": {
        const pending = this.pendingPublishes.get(msg.channel);
        if (pending) {
          this.pendingPublishes.delete(msg.channel);
          pending.reject(new Error(msg.message));
        }
        break;
      }

      case "message": {
        this.dispatchMessage(msg.channel, msg.payload);
        break;
      }

      case "pong": {
        const pending = this.pendingPings.get(msg.seq);
        if (pending) {
          this.pendingPings.delete(msg.seq);
          pending.resolve();
        }
        break;
      }

      case "error": {
        console.error("Server error:", msg.message);
        break;
      }
    }
  }

  private dispatchMessage(channel: string, payload: unknown): void {
    // Check each subscription pattern for a match
    for (const [pattern, handlers] of this.subscriptions) {
      if (this.patternMatches(pattern, channel)) {
        for (const handler of handlers) {
          try {
            handler(channel, payload);
          } catch (e) {
            console.error("Handler error:", e);
          }
        }
      }
    }
  }

  private patternMatches(pattern: string, channel: string): boolean {
    // Exact match
    if (pattern === channel) {
      return true;
    }

    // Wildcard match (pattern ends with .*)
    if (pattern.endsWith(".*")) {
      const prefix = pattern.slice(0, -1); // Keep the dot
      return channel.startsWith(prefix);
    }

    return false;
  }

  private handleDisconnect(): void {
    this.cleanup();

    if (this.config.autoReconnect) {
      this.setState("reconnecting");
      this.scheduleReconnect();
    } else {
      this.setState("disconnected");
    }
  }

  private scheduleReconnect(): void {
    const delay = Math.min(
      this.config.reconnectDelay * Math.pow(2, this.reconnectAttempts),
      this.config.maxReconnectDelay
    );
    this.reconnectAttempts++;

    this.reconnectTimer = setTimeout(async () => {
      try {
        await this.connect();
      } catch (e) {
        console.error("Reconnect failed:", e);
        if (this.config.autoReconnect) {
          this.scheduleReconnect();
        }
      }
    }, delay);
  }

  private startPingTimer(): void {
    this.pingTimer = setInterval(async () => {
      try {
        await this.ping();
      } catch (e) {
        console.error("Ping failed:", e);
        this.handleDisconnect();
      }
    }, this.config.pingInterval);
  }

  private cleanup(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }

    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }

    if (this.stream) {
      try {
        this.stream.readable.cancel();
        this.stream.writable.close();
      } catch {
        // Ignore errors during cleanup
      }
      this.stream = null;
    }

    if (this.transport) {
      try {
        this.transport.close();
      } catch {
        // Ignore errors during cleanup
      }
      this.transport = null;
    }

    // Reject all pending operations
    for (const pending of this.pendingSubscribes.values()) {
      pending.reject(new Error("Disconnected"));
    }
    this.pendingSubscribes.clear();

    for (const pending of this.pendingUnsubscribes.values()) {
      pending.reject(new Error("Disconnected"));
    }
    this.pendingUnsubscribes.clear();

    for (const pending of this.pendingPublishes.values()) {
      pending.reject(new Error("Disconnected"));
    }
    this.pendingPublishes.clear();

    for (const pending of this.pendingPings.values()) {
      pending.reject(new Error("Disconnected"));
    }
    this.pendingPings.clear();
  }

  private setState(state: ConnectionState, error?: Error): void {
    this.state = state;
    for (const handler of this.stateHandlers) {
      try {
        handler(state, error);
      } catch (e) {
        console.error("State handler error:", e);
      }
    }
  }
}

export { CatbusClient } from "./client.js";
export type {
  CatbusConfig,
  ClientMessage,
  ConnectionState,
  MessageHandler,
  ServerMessage,
  StateChangeHandler,
  Subscription,
  SubscribeOptions,
} from "./types.js";

// React hooks (optional import)
export {
  useCatbus,
  useSubscription,
  useLatestMessage,
  usePublish,
  useConnectionState,
} from "./react.js";

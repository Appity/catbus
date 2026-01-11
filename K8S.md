# Kubernetes Deployment

This guide covers deploying Catbus in Kubernetes, with specific guidance for handling WebTransport (QUIC/UDP) and WebSocket transports behind ingress controllers.

## The Transport Problem

Catbus supports two transports:

| Transport | Protocol | Port | Proxy Support |
|-----------|----------|------|---------------|
| **WebTransport** | QUIC over UDP | 4433 | Limited |
| **WebSocket** | HTTP/TCP | 8080 | Excellent |

**WebTransport** provides superior performance (multiplexed streams, no head-of-line blocking, 0-RTT), but QUIC/UDP proxying is poorly supported by ingress controllers.

**WebSocket** works through any HTTP-capable reverse proxy but loses WebTransport's performance benefits.

## Recommended Architecture

```
                    ┌─────────────────────────────────────────┐
                    │              Kubernetes                  │
                    │                                          │
  External          │   ┌─────────────┐    ┌───────────────┐  │
  Clients           │   │   Traefik   │    │    Catbus     │  │
       │            │   │   Ingress   │───▶│   Pod(s)      │  │
       │            │   │  (TCP:443)  │    │  ws: 8080     │  │
       │            │   └─────────────┘    │  wt: 4433     │  │
       ▼            │                      └───────────────┘  │
  ┌─────────┐       │                             ▲           │
  │   WSS   │───────┼─────────────────────────────┘           │
  │  :443   │       │                                          │
  └─────────┘       │   ┌─────────────┐    ┌───────────────┐  │
                    │   │  UDP Load   │    │    Catbus     │  │
  Internal          │   │  Balancer   │───▶│   Pod(s)      │  │
  Services ─────────┼──▶│  (UDP:4433) │    │  wt: 4433     │  │
                    │   └─────────────┘    └───────────────┘  │
                    │                                          │
                    └─────────────────────────────────────────┘
```

- **External clients**: Connect via WebSocket through Traefik (or any ingress)
- **Internal services**: Connect via WebTransport directly (pod-to-pod UDP works fine)

## Kubernetes Manifests

### Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: catbus
  labels:
    app: catbus
spec:
  replicas: 3
  selector:
    matchLabels:
      app: catbus
  template:
    metadata:
      labels:
        app: catbus
    spec:
      containers:
        - name: catbus
          image: ghcr.io/your-org/catbus:latest
          ports:
            - name: webtransport
              containerPort: 4433
              protocol: UDP
            - name: websocket
              containerPort: 8080
              protocol: TCP
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: catbus-secrets
                  key: database-url
            - name: CATBUS_SECRET
              valueFrom:
                secretKeyRef:
                  name: catbus-secrets
                  key: token-secret
            - name: CATBUS_CERT
              value: /etc/catbus/tls/tls.crt
            - name: CATBUS_KEY
              value: /etc/catbus/tls/tls.key
          volumeMounts:
            - name: tls
              mountPath: /etc/catbus/tls
              readOnly: true
          livenessProbe:
            httpGet:
              path: /health
              port: websocket
            initialDelaySeconds: 5
            periodSeconds: 10
          readinessProbe:
            httpGet:
              path: /health
              port: websocket
            initialDelaySeconds: 2
            periodSeconds: 5
      volumes:
        - name: tls
          secret:
            secretName: catbus-tls
```

### Services

```yaml
# WebSocket service for external access (via ingress)
apiVersion: v1
kind: Service
metadata:
  name: catbus-ws
spec:
  selector:
    app: catbus
  ports:
    - name: websocket
      port: 8080
      targetPort: websocket
      protocol: TCP
---
# WebTransport service for internal access
apiVersion: v1
kind: Service
metadata:
  name: catbus-wt
spec:
  selector:
    app: catbus
  ports:
    - name: webtransport
      port: 4433
      targetPort: webtransport
      protocol: UDP
```

### Secrets

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: catbus-secrets
type: Opaque
stringData:
  database-url: "postgres://user:pass@postgres:5432/catbus"
  token-secret: "your-secure-signing-secret"
```

## Traefik IngressRoute

### WebSocket via Traefik

```yaml
apiVersion: traefik.io/v1alpha1
kind: IngressRoute
metadata:
  name: catbus-websocket
spec:
  entryPoints:
    - websecure
  routes:
    - match: Host(`catbus.example.com`) && PathPrefix(`/ws`)
      kind: Rule
      services:
        - name: catbus-ws
          port: 8080
  tls:
    certResolver: letsencrypt
```

Or with standard Ingress:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: catbus-websocket
  annotations:
    kubernetes.io/ingress.class: traefik
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
spec:
  tls:
    - hosts:
        - catbus.example.com
      secretName: catbus-ingress-tls
  rules:
    - host: catbus.example.com
      http:
        paths:
          - path: /ws
            pathType: Prefix
            backend:
              service:
                name: catbus-ws
                port:
                  number: 8080
```

## Why Not Proxy WebTransport?

As of 2025, no major ingress controller properly supports WebTransport:

| Ingress | HTTP/3 | WebTransport | Notes |
|---------|--------|--------------|-------|
| Traefik | Yes | No | HTTP/3 for requests only, not QUIC streams |
| NGINX | Yes | No | Can't even reverse proxy HTTP/3 upstream |
| HAProxy | Yes | No | Only `h3` ALPN, no raw QUIC |
| Envoy | Yes | WIP | L4 QUIC listener still in development |
| Caddy | Yes | No | Deferred indefinitely |

WebTransport uses QUIC in ways that go beyond standard HTTP/3:
- Bidirectional streams
- Unreliable datagrams
- Multiple concurrent streams without head-of-line blocking

These features require QUIC-aware proxying (see [IETF MASQUE](https://datatracker.ietf.org/doc/draft-ietf-masque-quic-proxy/)), which isn't widely implemented.

## Direct WebTransport Access

If you need WebTransport for external clients (e.g., for performance-critical use cases), you have options:

### Option 1: UDP LoadBalancer (Cloud)

```yaml
apiVersion: v1
kind: Service
metadata:
  name: catbus-wt-external
  annotations:
    # AWS NLB
    service.beta.kubernetes.io/aws-load-balancer-type: "nlb"
spec:
  type: LoadBalancer
  selector:
    app: catbus
  ports:
    - name: webtransport
      port: 4433
      targetPort: webtransport
      protocol: UDP
```

This exposes WebTransport directly, bypassing the ingress.

### Option 2: Dual-Protocol Client

Configure clients to try WebTransport first, fall back to WebSocket:

```typescript
const client = new CatbusClient({
  // Try WebTransport first (direct UDP)
  url: 'https://catbus-wt.example.com:4433',
  // Fall back to WebSocket through ingress
  fallbackUrl: 'wss://catbus.example.com/ws',
  token: '...'
});
```

### Option 3: DNS-Based Routing

Use separate DNS records:
- `catbus.example.com` → Ingress (WebSocket)
- `catbus-direct.example.com` → UDP LoadBalancer (WebTransport)

Clients choose based on their network environment.

## Connection Affinity

For WebSocket connections, consider enabling session affinity if your application requires it:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: catbus-ws
spec:
  sessionAffinity: ClientIP
  sessionAffinityConfig:
    clientIP:
      timeoutSeconds: 3600
  # ...
```

Note: Catbus is designed to work without sticky sessions—messages are fanned out via PostgreSQL NOTIFY—but affinity can reduce reconnection overhead.

## Scaling Considerations

### Horizontal Pod Autoscaler

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: catbus
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: catbus
  minReplicas: 2
  maxReplicas: 10
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
```

### PostgreSQL Connection Pooling

With multiple Catbus replicas, use a connection pooler like PgBouncer:

```yaml
env:
  - name: DATABASE_URL
    value: "postgres://catbus:pass@pgbouncer:6432/catbus?sslmode=require"
```

## Health Checks

The WebSocket server exposes a `/health` endpoint:

```bash
curl http://catbus-pod:8080/health
# Returns: ok
```

Use this for:
- Kubernetes liveness/readiness probes
- Load balancer health checks
- Monitoring systems

## Monitoring

Catbus logs in JSON format when `RUST_LOG` is set appropriately:

```yaml
env:
  - name: RUST_LOG
    value: "catbus=info,tower_http=debug"
```

Integrate with your logging stack (Loki, Elasticsearch, etc.) for observability.

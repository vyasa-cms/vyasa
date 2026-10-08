# Deploy to Kubernetes

One Deployment of the official image, one PersistentVolumeClaim for
uploads and the search index, one Service. Verified on a
[kind](https://kind.sigs.k8s.io) cluster for 0.2.0; the manifests use
nothing cluster-specific.

**You bring:** a Postgres 15+ reachable from the cluster (a managed
database, or [CloudNativePG](https://cloudnative-pg.io) in-cluster) and an
Ingress or Gateway of your choosing in front of the `vyasa` Service.

## Apply

```bash
cp secret.example.yaml secret.yaml       # fill in DATABASE_URL and the rest
kubectl apply -f namespace.yaml
kubectl apply -f secret.yaml
kubectl apply -k .                       # pvc, deployment, service
kubectl -n vyasa rollout status deploy/vyasa
```

The first boot applies migrations and, because the Secret carries
`VYASA_ADMIN_EMAIL` and `VYASA_ADMIN_PASSWORD`, creates that administrator
and marks setup done. Sign in at `/admin`, then remove the two `ADMIN`
keys from the Secret and roll the Deployment (`kubectl -n vyasa rollout
restart deploy/vyasa`); once users exist they are ignored anyway.

Prefer the browser wizard? Leave the `ADMIN` keys out: the boot prints a
setup token to the log (`kubectl -n vyasa logs deploy/vyasa | grep stp_`)
for `/admin/setup`.

## What the manifests do

- `replicas: 1` and `strategy: Recreate`: one instance per site. The
  search index is local to the pod; the job queue is in Postgres, so a
  restart loses nothing.
- `readOnlyRootFilesystem: true`, no capabilities, non-root. The server
  writes only to `/data` (the PVC) and `/tmp` (an `emptyDir`), where
  `VYASA_RUN_DIR` keeps the setup token.
- `livenessProbe` on `/healthz`, `readinessProbe` and `startupProbe` on
  `/readyz` (database reachable, migrations applied). Traffic arrives only
  when the pod is ready.
- The PVC holds `/data/index` and `/data/media`. For media in object
  storage instead, add the `VYASA_STORAGE__*` keys to the Secret
  (`PROVIDER=s3`, `BUCKET`, `ENDPOINT`, `REGION`, `ACCESS_KEY_ID`,
  `SECRET_ACCESS_KEY`, `PATH_STYLE` for MinIO) and the PVC can shrink to
  the index alone.
- Resources: 256Mi/250m requested, 1Gi/1 CPU limit. A small site idles
  well under 200Mi; raise the limit for heavy media processing.

## Upgrading

Change the image tag in `deployment.yaml` and apply. Back up the
database first: migrations are forward-only. `Recreate` stops the old pod
before the new one starts, so the index on the PVC is never written by
two processes.

## Verified checklist (0.2.0, kind)

Apply, wait for Ready, port-forward, sign in with the environment
administrator, upload a file, delete the pod, confirm `/readyz` returns
200 on the replacement and search still answers.

import { Container, getContainer } from "@cloudflare/containers";

interface Env {
  VYASA: DurableObjectNamespace<VyasaContainer>;
  DATABASE_URL: string;
  VYASA_SECRET_KEY: string;
  R2_BUCKET: string;
  R2_ENDPOINT: string;
  R2_ACCESS_KEY_ID: string;
  R2_SECRET_ACCESS_KEY: string;
  VYASA_ADMIN_EMAIL?: string;
  VYASA_ADMIN_PASSWORD?: string;
}

export class VyasaContainer extends Container<Env> {
  defaultPort = 3000;
  // Every request renews this timer, so the container sleeps only after a
  // day without visitors; the next visitor pays a cold boot and an index
  // rebuild. (The library parses "<n>s|m|h" only.)
  sleepAfter = "24h";

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.envVars = {
      PORT: "3000",
      DATABASE_URL: env.DATABASE_URL,
      VYASA_SECRET_KEY: env.VYASA_SECRET_KEY,
      VYASA_RUN_DIR: "/tmp/vyasa-run",
      VYASA_INDEX_DIR: "/tmp/vyasa-index",
      VYASA_MEDIA_DIR: "/tmp/vyasa-media",
      VYASA_LOG__FORMAT: "json",
      VYASA_STORAGE__PROVIDER: "s3",
      VYASA_STORAGE__BUCKET: env.R2_BUCKET,
      VYASA_STORAGE__ENDPOINT: env.R2_ENDPOINT,
      VYASA_STORAGE__REGION: "auto",
      VYASA_STORAGE__ACCESS_KEY_ID: env.R2_ACCESS_KEY_ID,
      VYASA_STORAGE__SECRET_ACCESS_KEY: env.R2_SECRET_ACCESS_KEY,
      ...(env.VYASA_ADMIN_EMAIL && env.VYASA_ADMIN_PASSWORD
        ? { VYASA_ADMIN_EMAIL: env.VYASA_ADMIN_EMAIL, VYASA_ADMIN_PASSWORD: env.VYASA_ADMIN_PASSWORD }
        : {}),
    };
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Every request goes to the one container.
    return getContainer(env.VYASA, "site").fetch(request);
  },
};

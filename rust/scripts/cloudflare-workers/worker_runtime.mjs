export function createWorkerRuntime({ initWasm, wasmInput, WasmEdgeWorker, WasmHookInboxObject }) {
  let readyPromise;
  async function ensureInit() {
    if (!readyPromise) {
      readyPromise = Promise.resolve(initWasm(wasmInput)).then(() => new WasmEdgeWorker());
    }
    return readyPromise;
  }

  async function executeEffects(effects, env) {
    for (const eff of effects || []) {
      if (eff.kind === 'd1Query' && eff.plan) {
        const db = eff.plan.database === 'SAVINGS' ? env?.SAVINGS : env?.INDEX;
        if (db?.prepare) {
          await db.prepare(eff.plan.sql).bind(...(eff.plan.params || [])).run();
        }
      } else if (eff.kind === 'r2Op' && eff.op) {
        const b = eff.op.bucket === 'CLIENTS' ? env?.CLIENTS : env?.ARTIFACTS;
        if (b && eff.op.op === 'put') await b.put(eff.op.key, new Uint8Array(eff.op.body || []));
        if (b && eff.op.op === 'delete') await b.delete(eff.op.keys || []);
      } else if (eff.kind === 'hyperdriveQuery' && eff.plan && env?.RELAY_DB?.query) {
        await env.RELAY_DB.query(eff.plan.sql, eff.plan.params || []);
      } else if (eff.kind === 'durableObjectCall' && env?.HOOK_INBOX && eff.objectId) {
        const stub = env.HOOK_INBOX.get(env.HOOK_INBOX.idFromName(eff.objectId));
        await stub.fetch(new Request(`https://do.internal/${eff.method}`, { method: 'POST', body: JSON.stringify(eff.actor || {}) }));
      }
    }
  }

  class HookInboxObject {
    constructor(state, env) {
      this.state = state;
      this.env = env;
      this.actor = null;
    }
    async getActor(endpointKey = 'default') {
      await ensureInit();
      if (!this.actor) {
        const saved = await this.state?.storage?.get('actorState');
        this.actor = saved ? WasmHookInboxObject.from_state_json(saved) : new WasmHookInboxObject(endpointKey);
      }
      return this.actor;
    }
    async persist() {
      if (!this.actor || !this.state?.storage) return;
      await this.state.storage.put('actorState', this.actor.export_state_json());
      const alarmAt = this.actor.alarm_at_ms();
      if (alarmAt !== undefined && alarmAt !== null && this.state.storage.setAlarm) {
        await this.state.storage.setAlarm(Number(alarmAt));
      }
    }
    async fetch(request) {
      const url = new URL(request.url);
      const nowMs = Date.now();
      const actor = await this.getActor(url.searchParams.get('endpoint') || 'default');
      const rawBody = request.method === 'GET' || request.method === 'HEAD' ? null : await request.text();
      let bodyVal = null;
      if (rawBody) { try { bodyVal = JSON.parse(rawBody); } catch { bodyVal = rawBody; } }
      const res = JSON.parse(actor.fetch_json(JSON.stringify({
        method: request.method,
        path: url.pathname,
        headers: Object.fromEntries(request.headers.entries()),
        body: bodyVal,
        nowMs,
      })));
      await this.persist();
      return new Response(res.bodyText ?? JSON.stringify(res.body ?? {}), {
        status: res.status,
        headers: res.headers ?? { 'content-type': 'application/json' },
      });
    }
    async alarm() {
      const nowMs = Date.now();
      const actor = await this.getActor();
      const batch = JSON.parse(actor.begin_alarm_batch(nowMs));
      let delivered = 0;
      if (!batch.empty && batch.baseUrl) {
        for (const hook of batch.hooks || []) {
          const gen = Number(batch.targetGeneration);
          actor.record_outcome_json(JSON.stringify({ snapshotGeneration: gen, hookId: hook.id, outcome: 'delivered', nowMs }));
          delivered++;
        }
      }
      actor.finish_alarm_batch(delivered, 0, nowMs);
      await this.persist();
    }
  }

  return {
    HookInboxObject,
    default: {
      async fetch(request, env, _ctx) {
        const worker = await ensureInit();
        const url = new URL(request.url);
        const rawBody = request.method === 'GET' || request.method === 'HEAD' ? null : await request.text();
        let rateLimitAllowed = true;
        if (env?.RATE_LIMITER?.limit) {
          const r = await env.RATE_LIMITER.limit({ key: url.pathname });
          rateLimitAllowed = Boolean(r?.success ?? true);
        }
        const out = JSON.parse(worker.dispatch_with_bindings_json(JSON.stringify({
          method: request.method,
          path: url.pathname,
          query: url.search ? url.search.slice(1) : null,
          headers: Object.fromEntries(request.headers.entries()),
          body: rawBody,
          preloaded: { rateLimitAllowed },
          nowMs: Date.now(),
        })));
        await executeEffects(out.effects, env);
        return new Response(out.response.bodyText ?? '', {
          status: out.response.status,
          headers: out.response.headers ?? {},
        });
      },
      async scheduled(controller, env, _ctx) {
        const worker = await ensureInit();
        const report = JSON.parse(worker.handle_scheduled_json(controller.cron || '* * * * *', Date.now()));
        for (const plan of report.d1Plans || []) {
          const db = plan.database === 'SAVINGS' ? env?.SAVINGS : env?.INDEX;
          if (db?.prepare) {
            await db.prepare(plan.sql).bind(...(plan.params || [])).run();
          }
        }
        return report;
      },
    },
  };
}

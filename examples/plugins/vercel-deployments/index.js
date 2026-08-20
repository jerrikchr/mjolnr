#!/usr/bin/env node
/**
 * Vercel deployments example plugin — ADR-0016 JSON-RPC 2.0 over stdio.
 *
 * Reads newline-delimited JSON-RPC requests from stdin, writes responses to
 * stdout. Only VERCEL_TOKEN is available (injected by the host); the host's
 * scrubbed environment guarantees provider keys like OPENAI_API_KEY are absent.
 *
 * Keep this host minimal: it proves the subprocess host, Execute-pinned tools,
 * and observer hooks — nothing more.
 */
"use strict";

const readline = require("readline");

const API_BASE = process.env.VERCEL_API_BASE || "https://api.vercel.com";
const TOKEN = process.env.VERCEL_TOKEN || "";

const rl = readline.createInterface({ input: process.stdin, terminal: false });

function reply(id, result) {
  process.stdout.write(
    JSON.stringify({ jsonrpc: "2.0", id, result }) + "\n"
  );
}

function error(id, code, message) {
  process.stdout.write(
    JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } }) + "\n"
  );
}

async function vercelFetch(path) {
  if (!TOKEN) {
    throw new Error("VERCEL_TOKEN is not configured");
  }
  const url = `${API_BASE}${path}`;
  const res = await fetch(url, {
    headers: {
      Authorization: `Bearer ${TOKEN}`,
      Accept: "application/json",
      "User-Agent": "mjolnr",
    },
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`Vercel answered ${res.status} ${res.statusText}${text ? `: ${text.slice(0, 500)}` : ""}`);
  }
  return res.json();
}

rl.on("line", async (line) => {
  line = line.trim();
  if (!line) return;
  let req;
  try {
    req = JSON.parse(line);
  } catch {
    return;
  }
  const id = req.id;
  const method = req.method;
  const params = req.params || {};

  try {
    if (method === "initialize") {
      reply(id, { status: "ready", protocol_version: 1, plugin: "vercel.deployments" });
    } else if (method === "session_start") {
      reply(id, {
        annotations: ["Vercel deployments available via plugin:vercel.deployments:list_deployments"],
        notices: [],
      });
    } else if (method === "call_tool") {
      const name = params.name;
      const args = params.parameters || {};
      if (name === "list_deployments") {
        const projectId = args.project_id;
        if (!projectId || typeof projectId !== "string") {
          error(id, -32602, "project_id is required");
          return;
        }
        const data = await vercelFetch(`/v6/deployments?projectId=${encodeURIComponent(projectId)}&limit=10`);
        const deployments = (data.deployments || []).map((d) => ({
          id: d.uid || d.id || "",
          url: d.url || "",
          state: d.readyState || d.state || "UNKNOWN",
          createdAt: d.createdAt ?? null,
        }));
        reply(id, { deployments });
      } else if (name === "get_deployment") {
        const deploymentId = args.deployment_id;
        if (!deploymentId || typeof deploymentId !== "string") {
          error(id, -32602, "deployment_id is required");
          return;
        }
        if (/[\/#?%\s]/.test(deploymentId) || deploymentId.includes("..")) {
          error(id, -32602, "invalid deployment_id");
          return;
        }
        const data = await vercelFetch(`/v6/deployments/${encodeURIComponent(deploymentId)}`);
        reply(id, {
          deployment: {
            id: data.id || data.uid || deploymentId,
            url: data.url || "",
            state: data.readyState || data.state || "UNKNOWN",
            name: data.name || "",
            source: data.source || "",
            target: data.target || "",
            createdAt: data.createdAt ?? null,
          },
        });
      } else {
        error(id, -32601, `unknown tool ${name}`);
      }
    } else if (method === "shutdown") {
      reply(id, { status: "shutting down" });
      process.exit(0);
    } else {
      error(id, -32601, `unknown method ${method}`);
    }
  } catch (e) {
    error(id, -32603, e && e.message ? e.message : String(e));
  }
});

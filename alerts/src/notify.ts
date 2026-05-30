/**
 * Unified notification interface.
 * Supports email, Slack incoming webhooks, and Discord incoming webhooks.
 */

import { sendVerificationEmail, sendApyAlert } from "./email.ts";

export type ChannelType = "email" | "slack" | "discord";

export interface NotifyEnv {
  RESEND_API_KEY: string;
  RESEND_FROM: string;
}

export interface SendResult {
  ok: boolean;
  error?: string;
}

export interface Subscription {
  channel_type: ChannelType;
  /** email address for email channel; webhook URL for slack/discord */
  destination: string;
  unsub_token: string;
}

export interface AlertOpts {
  poolName: string;
  assetSymbol: string;
  leverage: number;
  netApy: number;
  supplyApr: number;
  borrowCost: number;
  appUrl: string;
  unsubscribeUrl: string;
}

// ── Slack ─────────────────────────────────────────────────────────────────────

async function postWebhook(url: string, body: object): Promise<SendResult> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text();
    return { ok: false, error: `Webhook ${res.status}: ${text}` };
  }
  return { ok: true };
}

function slackAlertPayload(opts: AlertOpts): object {
  return {
    text: `⚠️ *Negative APY Alert* — ${opts.assetSymbol} at ${opts.leverage}x on ${opts.poolName}`,
    blocks: [
      {
        type: "section",
        text: {
          type: "mrkdwn",
          text: `⚠️ *Negative APY Alert*\n${opts.assetSymbol} at ${opts.leverage}x on ${opts.poolName}\n\n• Net supply APR: *${opts.supplyApr.toFixed(2)}%*\n• Net borrow cost: *${opts.borrowCost.toFixed(2)}%*\n• *Net APY: ${opts.netApy.toFixed(2)}%*\n\n<${opts.appUrl}|Open Turbolong> | <${opts.unsubscribeUrl}|Unsubscribe>`,
        },
      },
    ],
  };
}

function discordAlertPayload(opts: AlertOpts): object {
  return {
    content: `⚠️ **Negative APY Alert** — ${opts.assetSymbol} at ${opts.leverage}x on ${opts.poolName}`,
    embeds: [
      {
        color: 0xff4d6a,
        fields: [
          { name: "Net supply APR", value: `${opts.supplyApr.toFixed(2)}%`, inline: true },
          { name: "Net borrow cost", value: `${opts.borrowCost.toFixed(2)}%`, inline: true },
          { name: `Net APY at ${opts.leverage}x`, value: `**${opts.netApy.toFixed(2)}%**`, inline: false },
        ],
        description: `[Open Turbolong](${opts.appUrl}) | [Unsubscribe](${opts.unsubscribeUrl})`,
      },
    ],
  };
}

// ── Verification ──────────────────────────────────────────────────────────────

export async function sendVerification(
  env: NotifyEnv,
  channel: ChannelType,
  destination: string,
  verifyUrl: string,
): Promise<SendResult> {
  if (channel === "email") {
    return sendVerificationEmail(env, destination, verifyUrl);
  }
  if (channel === "slack") {
    return postWebhook(destination, {
      text: `👋 Verify your Turbolong alert subscription: ${verifyUrl}`,
    });
  }
  // discord
  return postWebhook(destination, {
    content: `👋 Verify your Turbolong alert subscription: ${verifyUrl}`,
  });
}

// ── Alert ─────────────────────────────────────────────────────────────────────

export async function notify(
  env: NotifyEnv,
  sub: Subscription,
  opts: AlertOpts,
): Promise<SendResult> {
  if (sub.channel_type === "email") {
    return sendApyAlert(env, sub.destination, opts);
  }
  const payload =
    sub.channel_type === "slack"
      ? slackAlertPayload(opts)
      : discordAlertPayload(opts);
  return postWebhook(sub.destination, payload);
}

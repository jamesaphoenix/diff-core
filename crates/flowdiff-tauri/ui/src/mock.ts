/**
 * Mock data for demo mode — used when running outside Tauri (browser dev/Playwright).
 * Provides realistic fixture data so all UI states can be exercised without IPC.
 */
import type { AnalysisOutput, FileDiffContent } from "./types";

export const MOCK_ANALYSIS: AnalysisOutput = {
  version: "1.0.0",
  diff_source: {
    diff_type: "BranchComparison",
    base: "main",
    head: "feature/user-auth",
    base_sha: "a1b2c3d4e5f6",
    head_sha: "f6e5d4c3b2a1",
  },
  summary: {
    total_files_changed: 12,
    total_groups: 3,
    languages_detected: ["typescript", "python"],
    frameworks_detected: ["express", "prisma"],
  },
  groups: [
    {
      id: "group_1",
      name: "POST /api/users creation flow",
      entrypoint: {
        file: "src/routes/users.ts",
        symbol: "POST /api/users",
        entrypoint_type: "HttpRoute",
      },
      files: [
        {
          path: "src/routes/users.ts",
          flow_position: 0,
          role: "Entrypoint",
          changes: { additions: 35, deletions: 8 },
          symbols_changed: ["createUser", "validateInput"],
        },
        {
          path: "src/services/user-service.ts",
          flow_position: 1,
          role: "Service",
          changes: { additions: 42, deletions: 15 },
          symbols_changed: ["UserService.create", "UserService.validate"],
        },
        {
          path: "src/repositories/user-repo.ts",
          flow_position: 2,
          role: "Repository",
          changes: { additions: 18, deletions: 3 },
          symbols_changed: ["UserRepository.insert"],
        },
        {
          path: "src/models/user.ts",
          flow_position: 3,
          role: "Model",
          changes: { additions: 12, deletions: 0 },
          symbols_changed: ["User", "CreateUserInput"],
        },
      ],
      edges: [
        { from: "src/routes/users.ts::createUser", to: "src/services/user-service.ts::UserService.create", edge_type: "Calls" },
        { from: "src/services/user-service.ts::UserService.create", to: "src/repositories/user-repo.ts::UserRepository.insert", edge_type: "Calls" },
        { from: "src/routes/users.ts::createUser", to: "src/services/user-service.ts::UserService.validate", edge_type: "Calls" },
        { from: "src/repositories/user-repo.ts::UserRepository.insert", to: "src/models/user.ts::User", edge_type: "Writes" },
      ],
      risk_score: 0.82,
      review_order: 1,
    },
    {
      id: "group_2",
      name: "GET /api/auth/refresh token flow",
      entrypoint: {
        file: "src/routes/auth.ts",
        symbol: "GET /api/auth/refresh",
        entrypoint_type: "HttpRoute",
      },
      files: [
        {
          path: "src/routes/auth.ts",
          flow_position: 0,
          role: "Entrypoint",
          changes: { additions: 28, deletions: 12 },
          symbols_changed: ["refreshToken", "validateRefreshToken"],
        },
        {
          path: "src/services/auth-service.ts",
          flow_position: 1,
          role: "Service",
          changes: { additions: 55, deletions: 20 },
          symbols_changed: ["AuthService.refresh", "AuthService.rotateToken"],
        },
        {
          path: "src/middleware/rate-limit.ts",
          flow_position: 2,
          role: "Utility",
          changes: { additions: 8, deletions: 2 },
          symbols_changed: ["rateLimiter"],
        },
      ],
      edges: [
        { from: "src/routes/auth.ts::refreshToken", to: "src/services/auth-service.ts::AuthService.refresh", edge_type: "Calls" },
        { from: "src/services/auth-service.ts::AuthService.refresh", to: "src/services/auth-service.ts::AuthService.rotateToken", edge_type: "Calls" },
        { from: "src/routes/auth.ts::refreshToken", to: "src/middleware/rate-limit.ts::rateLimiter", edge_type: "Imports" },
      ],
      risk_score: 0.74,
      review_order: 2,
    },
    {
      id: "group_3",
      name: "Email notification worker",
      entrypoint: {
        file: "src/workers/email-worker.ts",
        symbol: "processEmailQueue",
        entrypoint_type: "QueueConsumer",
      },
      files: [
        {
          path: "src/workers/email-worker.ts",
          flow_position: 0,
          role: "Entrypoint",
          changes: { additions: 20, deletions: 5 },
          symbols_changed: ["processEmailQueue"],
        },
        {
          path: "src/services/email-service.ts",
          flow_position: 1,
          role: "Service",
          changes: { additions: 15, deletions: 8 },
          symbols_changed: ["EmailService.send", "EmailService.formatTemplate"],
        },
      ],
      edges: [
        { from: "src/workers/email-worker.ts::processEmailQueue", to: "src/services/email-service.ts::EmailService.send", edge_type: "Calls" },
        { from: "src/services/email-service.ts::EmailService.send", to: "src/services/email-service.ts::EmailService.formatTemplate", edge_type: "Calls" },
      ],
      risk_score: 0.35,
      review_order: 3,
    },
  ],
  infrastructure_group: {
    files: ["tsconfig.json", "package.json", ".eslintrc.json"],
    reason: "Not reachable from any detected entrypoint",
  },
  annotations: null,
};

export const MOCK_MERMAID: Record<string, string> = {
  group_1: `graph TD
    A["routes/users.ts::createUser"] --> B["user-service.ts::create"]
    A --> C["user-service.ts::validate"]
    B --> D["user-repo.ts::insert"]
    D --> E["models/user.ts::User"]
    style A fill:#89b4fa,stroke:#45475a,color:#1e1e2e
    style E fill:#a6e3a1,stroke:#45475a,color:#1e1e2e`,
  group_2: `graph TD
    A["routes/auth.ts::refreshToken"] --> B["auth-service.ts::refresh"]
    B --> C["auth-service.ts::rotateToken"]
    A --> D["rate-limit.ts::rateLimiter"]
    style A fill:#89b4fa,stroke:#45475a,color:#1e1e2e`,
  group_3: `graph TD
    A["email-worker.ts::processEmailQueue"] --> B["email-service.ts::send"]
    B --> C["email-service.ts::formatTemplate"]
    style A fill:#89b4fa,stroke:#45475a,color:#1e1e2e`,
};

export const MOCK_DIFFS: Record<string, FileDiffContent> = {
  "src/routes/users.ts": {
    path: "src/routes/users.ts",
    language: "typescript",
    old_content: `import { Router } from "express";
import { UserService } from "../services/user-service";

const router = Router();

router.post("/api/users", async (req, res) => {
  try {
    const user = await UserService.create(req.body);
    res.status(201).json(user);
  } catch (err) {
    res.status(400).json({ error: err.message });
  }
});

export default router;`,
    new_content: `import { Router } from "express";
import { UserService } from "../services/user-service";
import { validateInput } from "../middleware/validation";
import { CreateUserInput } from "../models/user";

const router = Router();

router.post("/api/users", validateInput(CreateUserInput), async (req, res) => {
  try {
    const validated = await UserService.validate(req.body);
    const user = await UserService.create(validated);
    res.status(201).json({
      id: user.id,
      email: user.email,
      created_at: user.created_at,
    });
  } catch (err) {
    if (err.code === "DUPLICATE_EMAIL") {
      res.status(409).json({ error: "Email already registered" });
    } else {
      res.status(400).json({ error: err.message });
    }
  }
});

export default router;`,
  },
  "src/services/user-service.ts": {
    path: "src/services/user-service.ts",
    language: "typescript",
    old_content: `import { UserRepository } from "../repositories/user-repo";

export class UserService {
  static async create(data: any) {
    return UserRepository.insert(data);
  }
}`,
    new_content: `import { UserRepository } from "../repositories/user-repo";
import { CreateUserInput, User } from "../models/user";
import { hashPassword } from "../utils/crypto";

export class UserService {
  static async validate(data: unknown): Promise<CreateUserInput> {
    if (!data || typeof data !== "object") {
      throw new Error("Invalid input");
    }
    const { email, password, name } = data as Record<string, string>;
    if (!email || !email.includes("@")) {
      throw new Error("Invalid email address");
    }
    if (!password || password.length < 8) {
      throw new Error("Password must be at least 8 characters");
    }
    return { email, password, name: name || "" };
  }

  static async create(input: CreateUserInput): Promise<User> {
    const hashedPassword = await hashPassword(input.password);
    return UserRepository.insert({
      ...input,
      password: hashedPassword,
    });
  }
}`,
  },
  "src/repositories/user-repo.ts": {
    path: "src/repositories/user-repo.ts",
    language: "typescript",
    old_content: `import { prisma } from "../db";

export class UserRepository {
  static async insert(data: any) {
    return prisma.user.create({ data });
  }
}`,
    new_content: `import { prisma } from "../db";
import { CreateUserInput, User } from "../models/user";

export class UserRepository {
  static async insert(data: CreateUserInput & { password: string }): Promise<User> {
    const existing = await prisma.user.findUnique({ where: { email: data.email } });
    if (existing) {
      const err = new Error("Email already registered");
      (err as any).code = "DUPLICATE_EMAIL";
      throw err;
    }
    return prisma.user.create({
      data: {
        email: data.email,
        password: data.password,
        name: data.name,
      },
    });
  }
}`,
  },
  "src/models/user.ts": {
    path: "src/models/user.ts",
    language: "typescript",
    old_content: `// No types defined yet`,
    new_content: `export interface User {
  id: string;
  email: string;
  name: string;
  created_at: Date;
  updated_at: Date;
}

export interface CreateUserInput {
  email: string;
  password: string;
  name: string;
}`,
  },
  "src/routes/auth.ts": {
    path: "src/routes/auth.ts",
    language: "typescript",
    old_content: `import { Router } from "express";
import { AuthService } from "../services/auth-service";

const router = Router();

router.get("/api/auth/refresh", async (req, res) => {
  const token = req.headers.authorization?.split(" ")[1];
  if (!token) {
    return res.status(401).json({ error: "No token provided" });
  }
  const newToken = await AuthService.refresh(token);
  res.json({ token: newToken });
});

export default router;`,
    new_content: `import { Router } from "express";
import { AuthService } from "../services/auth-service";
import { rateLimiter } from "../middleware/rate-limit";

const router = Router();

router.get("/api/auth/refresh", rateLimiter({ max: 10, window: 60 }), async (req, res) => {
  const token = req.headers.authorization?.split(" ")[1];
  if (!token) {
    return res.status(401).json({ error: "No token provided" });
  }
  try {
    const { accessToken, refreshToken } = await AuthService.refresh(token);
    res.json({
      access_token: accessToken,
      refresh_token: refreshToken,
      expires_in: 3600,
    });
  } catch (err) {
    if (err.message === "TOKEN_EXPIRED") {
      res.status(401).json({ error: "Refresh token expired, please login again" });
    } else {
      res.status(500).json({ error: "Internal server error" });
    }
  }
});

export default router;`,
  },
  "src/services/auth-service.ts": {
    path: "src/services/auth-service.ts",
    language: "typescript",
    old_content: `import jwt from "jsonwebtoken";

export class AuthService {
  static async refresh(token: string): Promise<string> {
    const payload = jwt.verify(token, process.env.JWT_SECRET!);
    return jwt.sign({ userId: payload.userId }, process.env.JWT_SECRET!, {
      expiresIn: "1h",
    });
  }
}`,
    new_content: `import jwt from "jsonwebtoken";
import { prisma } from "../db";

export class AuthService {
  static async refresh(token: string): Promise<{ accessToken: string; refreshToken: string }> {
    const payload = jwt.verify(token, process.env.JWT_SECRET!) as { userId: string };

    // Verify refresh token exists and is not revoked
    const stored = await prisma.refreshToken.findUnique({ where: { token } });
    if (!stored || stored.revoked) {
      throw new Error("TOKEN_EXPIRED");
    }

    // Rotate: revoke old, issue new
    await this.rotateToken(token, payload.userId);

    const accessToken = jwt.sign({ userId: payload.userId }, process.env.JWT_SECRET!, {
      expiresIn: "1h",
    });
    const refreshToken = jwt.sign({ userId: payload.userId }, process.env.JWT_REFRESH_SECRET!, {
      expiresIn: "30d",
    });

    return { accessToken, refreshToken };
  }

  static async rotateToken(oldToken: string, userId: string): Promise<void> {
    await prisma.$transaction([
      prisma.refreshToken.update({
        where: { token: oldToken },
        data: { revoked: true, revokedAt: new Date() },
      }),
      prisma.refreshToken.create({
        data: { userId, token: oldToken, expiresAt: new Date(Date.now() + 30 * 24 * 60 * 60 * 1000) },
      }),
    ]);
  }
}`,
  },
  "src/middleware/rate-limit.ts": {
    path: "src/middleware/rate-limit.ts",
    language: "typescript",
    old_content: `export function rateLimiter() {
  // placeholder
  return (req: any, res: any, next: any) => next();
}`,
    new_content: `interface RateLimitOptions {
  max: number;
  window: number; // seconds
}

const store = new Map<string, { count: number; resetAt: number }>();

export function rateLimiter(opts: RateLimitOptions) {
  return (req: any, res: any, next: any) => {
    const key = req.ip;
    const now = Date.now();
    const entry = store.get(key);

    if (!entry || now > entry.resetAt) {
      store.set(key, { count: 1, resetAt: now + opts.window * 1000 });
      return next();
    }

    if (entry.count >= opts.max) {
      return res.status(429).json({ error: "Rate limit exceeded" });
    }

    entry.count++;
    next();
  };
}`,
  },
  "src/workers/email-worker.ts": {
    path: "src/workers/email-worker.ts",
    language: "typescript",
    old_content: `import { EmailService } from "../services/email-service";

export async function processEmailQueue(job: any) {
  await EmailService.send(job.data);
}`,
    new_content: `import { EmailService } from "../services/email-service";

interface EmailJob {
  to: string;
  template: string;
  data: Record<string, unknown>;
  priority?: "high" | "normal" | "low";
}

export async function processEmailQueue(job: { data: EmailJob }) {
  const { to, template, data, priority } = job.data;

  if (priority === "high") {
    await EmailService.send({ to, template, data, immediate: true });
  } else {
    await EmailService.send({ to, template, data });
  }
}`,
  },
  "src/services/email-service.ts": {
    path: "src/services/email-service.ts",
    language: "typescript",
    old_content: `export class EmailService {
  static async send(data: any) {
    // send email
  }
}`,
    new_content: `import { templates } from "../templates";

interface SendOptions {
  to: string;
  template: string;
  data: Record<string, unknown>;
  immediate?: boolean;
}

export class EmailService {
  static async send(opts: SendOptions): Promise<void> {
    const html = this.formatTemplate(opts.template, opts.data);
    // Implementation: send via provider
  }

  static formatTemplate(name: string, data: Record<string, unknown>): string {
    const tpl = templates[name];
    if (!tpl) throw new Error(\`Unknown template: \${name}\`);
    return tpl.replace(/\\{\\{(\\w+)\\}\\}/g, (_, key) => String(data[key] ?? ""));
  }
}`,
  },
};

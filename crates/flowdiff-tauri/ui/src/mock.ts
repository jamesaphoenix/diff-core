/**
 * Mock data for demo mode — used when running outside Tauri (browser dev/Playwright).
 * Provides realistic fixture data so all UI states can be exercised without IPC.
 */
import type { AnalysisOutput, FileDiffContent, LlmSettings, Pass1Response, Pass2Response, RefinementResult, RepoInfo } from "./types";

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

export const MOCK_PASS1: Pass1Response = {
  groups: [
    {
      id: "group_1",
      name: "User creation API with validation and persistence",
      summary:
        "Adds input validation middleware, typed DTOs, and duplicate-email checking to the POST /api/users creation flow. The route handler now validates input before passing to the service layer, which hashes passwords before persisting via Prisma.",
      review_order_rationale:
        "Review first \u2014 this group changes the public API contract and touches the persistence layer (schema-adjacent). Downstream auth and email flows may depend on user creation succeeding correctly.",
      risk_flags: ["schema_change", "auth_adjacent", "public_api_change"],
    },
    {
      id: "group_2",
      name: "Auth token refresh with rotation and rate limiting",
      summary:
        "Implements rotating refresh tokens: old tokens are revoked on refresh, and a new refresh token is issued alongside the access token. Adds rate limiting middleware to prevent brute-force attacks on the refresh endpoint.",
      review_order_rationale:
        "Review second \u2014 auth token rotation is a security-critical change. A bug here could lock users out or allow token reuse after revocation.",
      risk_flags: ["auth_change", "security_critical", "breaking_api"],
    },
    {
      id: "group_3",
      name: "Email worker typed interface and priority routing",
      summary:
        "Adds TypeScript interfaces to the email worker queue consumer and introduces priority-based routing (high-priority emails are sent immediately). The email service now uses named templates with variable substitution.",
      review_order_rationale:
        "Review last \u2014 lowest risk. Changes are additive (new types, new feature) and isolated to the background worker pipeline.",
      risk_flags: [],
    },
  ],
  overall_summary:
    "This PR strengthens the user-facing API layer with input validation and auth hardening (rotating refresh tokens + rate limiting), then adds typed email templates to the background worker. The highest-risk changes are in auth token rotation \u2014 review the transaction logic carefully.",
  suggested_review_order: ["group_1", "group_2", "group_3"],
};

export const MOCK_PASS2: Record<string, Pass2Response> = {
  group_1: {
    group_id: "group_1",
    flow_narrative:
      "Request enters at POST /api/users route handler, which now runs the validateInput middleware (using the CreateUserInput schema). The handler calls UserService.validate() for business-rule validation, then UserService.create() which hashes the password and delegates to UserRepository.insert(). The repository checks for duplicate emails via Prisma before persisting. Error responses are now differentiated (409 for duplicates vs 400 for validation).",
    file_annotations: [
      {
        file: "src/routes/users.ts",
        role_in_flow: "Entrypoint \u2014 HTTP route handler for user creation",
        changes_summary:
          "Added validateInput middleware, typed response fields (id, email, created_at), and duplicate-email error handling (409 status).",
        risks: [
          "The validateInput middleware runs before the handler but errors from it may not match the handler's error format.",
        ],
        suggestions: [
          "Consider a shared error-formatting middleware to ensure consistent error shapes across all routes.",
        ],
      },
      {
        file: "src/services/user-service.ts",
        role_in_flow: "Service layer \u2014 validation and orchestration",
        changes_summary:
          "New validate() method with email format and password length checks. create() now hashes the password before persisting.",
        risks: [
          "Password hashing is imported from ../utils/crypto but the hash function is not shown \u2014 ensure it uses bcrypt/scrypt/argon2, not MD5/SHA.",
        ],
        suggestions: [],
      },
      {
        file: "src/repositories/user-repo.ts",
        role_in_flow: "Persistence layer \u2014 Prisma database operations",
        changes_summary:
          "Added duplicate-email check via findUnique before create. Throws custom DUPLICATE_EMAIL error code.",
        risks: [
          "Race condition: two concurrent requests with the same email could both pass the findUnique check. Rely on a DB unique constraint instead of application-level checks.",
        ],
        suggestions: [
          "Add a unique constraint on the email column and catch the Prisma unique constraint violation error instead of the findUnique check.",
        ],
      },
      {
        file: "src/models/user.ts",
        role_in_flow: "Data model \u2014 TypeScript type definitions",
        changes_summary:
          "New User and CreateUserInput interfaces defining the shape of user data.",
        risks: [],
        suggestions: [],
      },
    ],
    cross_cutting_concerns: [
      "Error handling: The route handler catches DUPLICATE_EMAIL but other error codes from the service/repo layers are not handled \u2014 they fall through to a generic 400.",
      "The password field is included in CreateUserInput but should never appear in API responses. Consider a separate output DTO.",
    ],
  },
  group_2: {
    group_id: "group_2",
    flow_narrative:
      "Request enters at GET /api/auth/refresh, rate limited to 10 requests per 60-second window per IP. The handler extracts the bearer token, then calls AuthService.refresh(). The service verifies the token via JWT, checks it against the database (revocation check), rotates the token pair (revoke old, issue new), and returns both access and refresh tokens.",
    file_annotations: [
      {
        file: "src/routes/auth.ts",
        role_in_flow: "Entrypoint \u2014 auth refresh endpoint with rate limiting",
        changes_summary:
          "Added rateLimiter middleware, destructured response to return both tokens + expires_in, added TOKEN_EXPIRED error handling.",
        risks: [],
        suggestions: [],
      },
      {
        file: "src/services/auth-service.ts",
        role_in_flow: "Service layer \u2014 token verification, rotation, and issuance",
        changes_summary:
          "Token refresh now verifies against DB (revocation check), rotates via $transaction (revoke old + create new), and returns both token types.",
        risks: [
          "The rotateToken method creates a new refresh token row with the OLD token value (line: token: oldToken). This looks like a bug \u2014 the new row should contain a newly generated token, not the old one.",
          "JWT_REFRESH_SECRET is used for refresh tokens but there is no rotation of this secret. If compromised, all refresh tokens are vulnerable.",
        ],
        suggestions: [
          "Generate a fresh random token for the new refresh token row instead of reusing oldToken.",
          "Consider adding a token family ID to detect token reuse attacks (if an old token is used after rotation, revoke the entire family).",
        ],
      },
      {
        file: "src/middleware/rate-limit.ts",
        role_in_flow: "Utility \u2014 in-memory rate limiter",
        changes_summary:
          "Upgraded from no-op to a functional in-memory rate limiter with configurable max and window.",
        risks: [
          "In-memory store is not shared across multiple server instances. In a multi-process or clustered deployment, each instance tracks separately.",
        ],
        suggestions: [
          "Consider Redis-backed rate limiting for production, or document the single-instance limitation.",
        ],
      },
    ],
    cross_cutting_concerns: [
      "The rate limiter uses req.ip which may be the load balancer's IP behind a reverse proxy. Ensure trust-proxy is configured in Express.",
    ],
  },
};

export const MOCK_REFINEMENT: RefinementResult = {
  refined_groups: [
    {
      id: "group_1",
      name: "User creation API with validation and persistence",
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
      ],
      edges: [
        { from: "src/routes/users.ts::createUser", to: "src/services/user-service.ts::UserService.create", edge_type: "Calls" },
        { from: "src/services/user-service.ts::UserService.create", to: "src/repositories/user-repo.ts::UserRepository.insert", edge_type: "Calls" },
      ],
      risk_score: 0.82,
      review_order: 1,
    },
    {
      id: "group_refined_1",
      name: "User data model definitions",
      entrypoint: null,
      files: [
        {
          path: "src/models/user.ts",
          flow_position: 0,
          role: "Model",
          changes: { additions: 12, deletions: 0 },
          symbols_changed: ["User", "CreateUserInput"],
        },
      ],
      edges: [],
      risk_score: 0.3,
      review_order: 2,
    },
    {
      id: "group_2",
      name: "Auth token refresh with rotation and rate limiting",
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
      ],
      risk_score: 0.74,
      review_order: 3,
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
      ],
      risk_score: 0.35,
      review_order: 4,
    },
  ],
  infrastructure_group: {
    files: ["tsconfig.json", "package.json", ".eslintrc.json"],
    reason: "Not reachable from any detected entrypoint",
  },
  refinement_response: {
    splits: [
      {
        source_group_id: "group_1",
        new_groups: [
          { name: "User creation API with validation and persistence", files: ["src/routes/users.ts", "src/services/user-service.ts", "src/repositories/user-repo.ts"] },
          { name: "User data model definitions", files: ["src/models/user.ts"] },
        ],
        reason: "The User/CreateUserInput type definitions are pure data models used across multiple flows \u2014 separating them from the API handler chain makes the review clearer",
      },
    ],
    merges: [],
    re_ranks: [
      { group_id: "group_2", new_position: 3, reason: "Review auth after user creation since it depends on user model" },
    ],
    reclassifications: [],
    reasoning: "Split the user creation group to isolate pure type definitions. Re-ranked auth flow after user creation for dependency order.",
  },
  provider: "anthropic",
  model: "claude-sonnet-4-6",
  had_changes: true,
};

export const MOCK_LLM_SETTINGS: LlmSettings = {
  annotations_enabled: true,
  refinement_enabled: false,
  provider: "anthropic",
  model: "claude-sonnet-4-6",
  api_key_source: "ANTHROPIC_API_KEY",
  has_api_key: true,
  refinement_provider: "anthropic",
  refinement_model: "claude-sonnet-4-6",
  refinement_max_iterations: 1,
};

export const MOCK_REPO_INFO: RepoInfo = {
  current_branch: "feature/user-auth",
  default_branch: "main",
  branches: [
    { name: "feature/user-auth", is_current: true, has_upstream: true },
    { name: "main", is_current: false, has_upstream: true },
    { name: "develop", is_current: false, has_upstream: true },
    { name: "feature/dashboard", is_current: false, has_upstream: false },
    { name: "fix/login-bug", is_current: false, has_upstream: false },
    { name: "release/v2.0", is_current: false, has_upstream: true },
  ],
  worktrees: [
    { path: "/demo/repo", branch: "feature/user-auth", is_main: true },
  ],
  status: {
    branch: "feature/user-auth",
    upstream: "origin/feature/user-auth",
    ahead: 3,
    behind: 0,
  },
};

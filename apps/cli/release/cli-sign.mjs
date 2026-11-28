#!/usr/bin/env node
/**
 * Signs the native `nexus` CLI's release binaries with the release key in Cloud KMS, and checks the signatures.
 * Used by .github/workflows/cli-release.yml (`release`). The keys it checks are in apps/cli/keys/.
 *
 *   node scripts/cli-sign.mjs sign --dir <binaries> --out <signatures> --version 0.9.0+abc1234 \
 *     --kms-key projects/…/cryptoKeyVersions/1 --pub <keys>/primary.pub --keys <keys>
 *   node scripts/cli-sign.mjs verify --dir <binaries> --sigs <signatures> --version 0.9.0+abc1234 --keys <keys>
 *   node scripts/cli-sign.mjs pubkey --pem <public key from KMS> --key-id <16 hex>
 *   node scripts/cli-sign.mjs self-test
 *
 * Each `nexus-<target>` in `<dir>` gets `<out>/nexus-<target>.sig` in the Tauri signer's format, the one
 * nexus-desktop-updates' `verify_signed_file` reads: the base64 of a minisign signature of the file's BLAKE2b-512 hash,
 * whose trusted comment names the file and the version (`file:nexus-linux-x64`, `version:0.9.0+abc1234`). KMS signs
 * Ed25519 over the bytes it is sent, so it signs the hash, then the signature with the trusted comment, as minisign does.
 *
 * `sign` reads an access token for a principal with `roles/cloudkms.signer` on `--kms-key` from KMS_ACCESS_TOKEN. It
 * runs the self-test first, refuses a signature that `--pub` does not verify, and finally checks every binary against
 * the keys in `--keys`, as the CLI will. A `.pub` there is the base64 of a minisign public key file, which `pubkey`
 * makes from the PEM that `gcloud kms keys versions get-public-key` writes.
 */
import {
  createHash,
  createPublicKey,
  generateKeyPairSync,
  randomBytes,
  sign as edSign,
  verify as edVerify,
} from "node:crypto";
import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const NAME = /^nexus-([a-z0-9]{1,16}-[a-z0-9]{1,16})(\.exe)?$/;
const SPKI_ED25519 = Buffer.from("302a300506032b6570032100", "hex");

const FIXTURE = {
  pub: "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDJFQkI1MkZEQjA3MEVDQjEKUldTeDdIQ3cvVks3TGh6L0xhQWtZSXlYOHpaQ3A3bGlzSi9SZTNpRmlOUGtpdlVzQ0lUWlBMbU8K",
  sig: "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIG1pbmlzaWduIHNlY3JldCBrZXkKUlVTeDdIQ3cvVks3TGdqMkdZK2p2YlpwY2ZxNktZaXI4cTc4M0RFOVowdWhzLytib25XWVNDcUR4MTdtWFEwMnF3YXhMbzBIY3lOemJSSlRGM1NHK2Jwa1NGd2xDOUN3RXdZPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkyNzg1NzA0CWZpbGU6dXBkYXRlLnR4dAl2ZXJzaW9uOjEuMC4wCkJZcGRwT2NBdEhoYkh3Vm12U0NpWG1pR3NDYWtpdC9XeEhnd2xVWXB1MEZFWWs4WElPNTZYNHoweGJ5YWUvRkdsUFlVS2tnaENHMGFPN3JrbVNpL0RBPT0K",
  data: "RmFrZSBkZXNrdG9wIHVwZGF0ZSBmb3Igc2lnbmF0dXJlIHZlcmlmaWNhdGlvbiB0ZXN0cy4gTmV2ZXIgaW5zdGFsbC4K",
  file: "update.txt",
  version: "1.0.0",
};

const base64 = (bytes) => Buffer.from(bytes).toString("base64");
const unbase64 = (text) => Buffer.from(text.trim(), "base64");
const blake2b512 = (bytes) => createHash("blake2b512").update(bytes).digest();
const ed25519Key = (raw) => createPublicKey({ key: Buffer.concat([SPKI_ED25519, raw]), format: "der", type: "spki" });

export function formatPublicKey(keyId, raw) {
  const id = Buffer.from(keyId).reverse().toString("hex").toUpperCase();
  const line = base64(Buffer.concat([Buffer.from("Ed"), keyId, raw]));
  return base64(Buffer.from(`untrusted comment: minisign public key: ${id}\n${line}\n`));
}

export function parsePublicKey(text) {
  const lines = unbase64(text).toString("utf8").split("\n");
  const bin = unbase64(lines[1] ?? "");
  if (!lines[0].startsWith("untrusted comment: ") || bin.length !== 42 || bin.subarray(0, 2).toString() !== "Ed") {
    throw new Error("not the base64 of an Ed25519 minisign public key");
  }
  return { keyId: bin.subarray(2, 10), raw: bin.subarray(10) };
}

function formatSignature(keyId, signature, trusted, global) {
  const line = base64(Buffer.concat([Buffer.from("ED"), keyId, signature]));
  const text = `untrusted comment: signature from the nexus CLI release key\n${line}\ntrusted comment: ${trusted}\n${base64(global)}\n`;
  return base64(Buffer.from(text));
}

function parseSignature(text) {
  const lines = unbase64(text).toString("utf8").split("\n");
  const bin = unbase64(lines[1] ?? "");
  const global = unbase64(lines[3] ?? "");
  if (
    !lines[0].startsWith("untrusted comment: ") ||
    !lines[2]?.startsWith("trusted comment: ") ||
    bin.length !== 74 ||
    global.length !== 64
  ) {
    throw new Error("not the base64 of a minisign signature");
  }
  return {
    algorithm: bin.subarray(0, 2).toString(),
    keyId: bin.subarray(2, 10),
    signature: bin.subarray(10),
    trusted: lines[2].slice("trusted comment: ".length),
    global,
  };
}

export function verifySigned(keys, text, version, file, bytes) {
  let parsed;
  try {
    parsed = parseSignature(text);
  } catch {
    return false;
  }
  const key = keys.find((candidate) => candidate.keyId.equals(parsed.keyId));
  const message = { ED: blake2b512(bytes), Ed: bytes }[parsed.algorithm];
  if (!key || !message) return false;
  const publicKey = ed25519Key(key.raw);
  const signed = Buffer.concat([parsed.signature, Buffer.from(parsed.trusted)]);
  if (!edVerify(null, message, publicKey, parsed.signature) || !edVerify(null, signed, publicKey, parsed.global)) {
    return false;
  }
  const fields = parsed.trusted.split("\t");
  const signedVersion = fields.find((field) => field.startsWith("version:"))?.slice("version:".length);
  return signedVersion === version && fields.includes(`file:${file}`);
}

export async function signFile(signer, keyId, bytes, file, version) {
  const signature = await signer(blake2b512(bytes));
  const trusted = `timestamp:${Math.floor(Date.now() / 1000)}\tfile:${file}\tversion:${version}`;
  const global = await signer(Buffer.concat([signature, Buffer.from(trusted)]));
  return formatSignature(keyId, signature, trusted, global);
}

export function localSigner(privateKey) {
  return async (data) => edSign(null, data, privateKey);
}

function kmsSigner(name, token) {
  return async (data) => {
    const response = await fetch(`https://cloudkms.googleapis.com/v1/${name}:asymmetricSign`, {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: JSON.stringify({ data: base64(data) }),
      signal: AbortSignal.timeout(30_000),
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(`KMS answered ${response.status}: ${body.error?.message ?? "no message"}`);
    if (body.name !== name) throw new Error(`KMS signed with ${body.name}, not ${name}`);
    const signature = unbase64(body.signature ?? "");
    if (signature.length !== 64) throw new Error("KMS answered without an Ed25519 signature");
    return signature;
  };
}

export async function selfTest() {
  const fixtureKeys = [parsePublicKey(FIXTURE.pub)];
  const data = unbase64(FIXTURE.data);
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(SPKI_ED25519.length);
  const keyId = randomBytes(8);
  const ownKeys = [parsePublicKey(formatPublicKey(keyId, raw))];
  const own = await signFile(localSigner(privateKey), keyId, data, "nexus-linux-x64", "0.9.0+abc1234");
  const checks = [
    ["the Tauri signer's signature verifies", verifySigned(fixtureKeys, FIXTURE.sig, FIXTURE.version, FIXTURE.file, data)],
    ["a changed file fails", !verifySigned(fixtureKeys, FIXTURE.sig, FIXTURE.version, FIXTURE.file, Buffer.concat([data, data]))],
    ["another version fails", !verifySigned(fixtureKeys, FIXTURE.sig, "1.0.1", FIXTURE.file, data)],
    ["another file name fails", !verifySigned(fixtureKeys, FIXTURE.sig, FIXTURE.version, "another.txt", data)],
    ["this script's signature verifies", verifySigned(ownKeys, own, "0.9.0+abc1234", "nexus-linux-x64", data)],
    ["it fails under another key", !verifySigned(fixtureKeys, own, "0.9.0+abc1234", "nexus-linux-x64", data)],
    ["it fails without the build", !verifySigned(ownKeys, own, "0.9.0", "nexus-linux-x64", data)],
  ];
  const failed = checks.filter(([, ok]) => !ok).map(([check]) => check);
  if (failed.length > 0) throw new Error(`self-test failed: ${failed.join("; ")}`);
}

function arg(name) {
  const index = process.argv.indexOf(`--${name}`);
  return index > 0 ? process.argv[index + 1] : undefined;
}

function required(names) {
  const values = Object.fromEntries(names.map((name) => [name, arg(name)]));
  const missing = names.filter((name) => !values[name]);
  if (missing.length > 0) throw new Error(`missing ${missing.map((name) => `--${name}`).join(", ")}`);
  return values;
}

function binaries(dir) {
  const files = readdirSync(dir)
    .sort()
    .filter((file) => NAME.test(file) && statSync(join(dir, file)).isFile());
  if (files.length === 0) throw new Error(`no nexus-<target> binary in ${dir}`);
  return files;
}

function readKeys(dir) {
  const keys = readdirSync(dir)
    .filter((file) => file.endsWith(".pub"))
    .map((file) => parsePublicKey(readFileSync(join(dir, file), "utf8")));
  if (keys.length === 0) throw new Error(`no .pub in ${dir}`);
  return keys;
}

function verifyDir({ dir, sigs, version, keys }) {
  const trusted = readKeys(keys);
  for (const file of binaries(dir)) {
    const text = readFileSync(join(sigs, `${file}.sig`), "utf8");
    if (!verifySigned(trusted, text, version, file, readFileSync(join(dir, file)))) {
      throw new Error(`${file}.sig does not verify for ${version} with the keys in ${keys}`);
    }
    console.log(`${file}.sig verifies`);
  }
}

async function signDir({ dir, out, version, "kms-key": kmsKey, pub, keys }) {
  await selfTest();
  const token = process.env.KMS_ACCESS_TOKEN;
  if (!token) throw new Error("KMS_ACCESS_TOKEN is not set");
  const signing = parsePublicKey(readFileSync(pub, "utf8"));
  if (!readKeys(keys).some((key) => key.keyId.equals(signing.keyId))) throw new Error(`${pub} is not one of ${keys}`);
  const signer = kmsSigner(kmsKey, token);
  mkdirSync(out, { recursive: true });
  for (const file of binaries(dir)) {
    const bytes = readFileSync(join(dir, file));
    const signature = await signFile(signer, signing.keyId, bytes, file, version);
    if (!verifySigned([signing], signature, version, file, bytes)) {
      throw new Error(`KMS's signature of ${file} does not verify with ${pub}: is that the key ${kmsKey}?`);
    }
    writeFileSync(join(out, `${file}.sig`), signature);
  }
  verifyDir({ dir, sigs: out, version, keys });
}

function pubkey({ pem, "key-id": keyIdHex }) {
  if (!/^[0-9A-Fa-f]{16}$/.test(keyIdHex)) throw new Error("--key-id must be 16 hex digits");
  const der = createPublicKey(readFileSync(pem)).export({ format: "der", type: "spki" });
  if (der.length !== SPKI_ED25519.length + 32 || !der.subarray(0, SPKI_ED25519.length).equals(SPKI_ED25519)) {
    throw new Error(`${pem} is not an Ed25519 public key`);
  }
  return formatPublicKey(Buffer.from(keyIdHex, "hex").reverse(), der.subarray(SPKI_ED25519.length));
}

async function main() {
  const command = process.argv[2];
  if (command === "self-test") {
    await selfTest();
    console.log("self-test passed");
  } else if (command === "pubkey") {
    process.stdout.write(`${pubkey(required(["pem", "key-id"]))}\n`);
  } else if (command === "sign") {
    await signDir(required(["dir", "out", "version", "kms-key", "pub", "keys"]));
  } else if (command === "verify") {
    verifyDir(required(["dir", "sigs", "version", "keys"]));
  } else {
    console.error("usage: cli-sign.mjs sign | verify | pubkey | self-test (see the comment at the top)");
    process.exit(2);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(`cli-sign: ${error.message}`);
    process.exit(1);
  });
}

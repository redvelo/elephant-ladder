import { constants, readFileSync } from "node:fs";
import { expect, test, vi } from "vitest";
import { androidJavaEnv, NDK_VERSION } from "../../scripts/android-java.mjs";

const properties = (version = "26", home = "/opt/System Java") => ({
  status: 0,
  stderr: `Property settings:\n    java.home = ${home}\n    java.specification.version = ${version}\n`,
  stdout: "",
});

function probe(
  result: {
    status: number | null;
    stderr: string;
    stdout: string;
    error?: Error;
    signal?: string;
  } = properties(),
) {
  const io = {
    run: vi.fn((..._args: unknown[]) => result),
    access: vi.fn(),
    platform: "linux" as NodeJS.Platform,
  };
  return io as typeof io & NonNullable<Parameters<typeof androidJavaEnv>[2]>;
}

test("PATH Java 26 supplies a child-only JAVA_HOME, with bounded probing", () => {
  const env = Object.freeze({ PATH: "/system/bin", ANDROID_HOME: "/system/sdk", TOKEN: "private" });
  const io = probe();
  expect(androidJavaEnv(["android", "dev", "--no-watch"], env, io)).toEqual({
    ...env,
    JAVA_HOME: "/opt/System Java",
    ANDROID_HOME: "/system/sdk",
    ANDROID_SDK_ROOT: "/system/sdk",
    NDK_HOME: `/system/sdk/ndk/${NDK_VERSION}`,
    ANDROID_NDK_HOME: `/system/sdk/ndk/${NDK_VERSION}`,
    ANDROID_NDK_ROOT: `/system/sdk/ndk/${NDK_VERSION}`,
  });
  expect(env).not.toHaveProperty("JAVA_HOME");
  expect(io.run).toHaveBeenCalledExactlyOnceWith(
    "java",
    ["-XshowSettings:properties", "-version"],
    {
      env,
      encoding: "utf8",
      timeout: 10_000,
      maxBuffer: 1024 * 1024,
      windowsHide: true,
    },
  );
  expect(io.access).toHaveBeenCalledWith("/opt/System Java/bin/javac", constants.X_OK);
  expect(io.access).toHaveBeenCalledWith(
    `/system/sdk/ndk/${NDK_VERSION}/ndk-build`,
    constants.X_OK,
  );
});

test("explicit JAVA_HOME wins over PATH and is preserved", () => {
  const env = Object.freeze({
    JAVA_HOME: "/chosen JDK",
    ANDROID_HOME: "/system/sdk",
    PATH: "/system/bin",
  });
  const io = probe(properties("25", "/resolved/jdk"));
  const selected = androidJavaEnv(["android", "build"], env, io);
  expect(selected).toEqual({
    ...env,
    ANDROID_SDK_ROOT: "/system/sdk",
    ANDROID_HOME: "/system/sdk",
    NDK_HOME: `/system/sdk/ndk/${NDK_VERSION}`,
    ANDROID_NDK_HOME: `/system/sdk/ndk/${NDK_VERSION}`,
    ANDROID_NDK_ROOT: `/system/sdk/ndk/${NDK_VERSION}`,
  });
  expect(selected).not.toBe(env);
  expect(io.run.mock.calls[0]?.[0]).toBe("/chosen JDK/bin/java");
  expect(io.access).toHaveBeenCalledWith("/chosen JDK/bin/javac", constants.X_OK);
});

test.each(["", "   "])("empty JAVA_HOME %j falls back to PATH", (JAVA_HOME) => {
  const io = probe();
  expect(androidJavaEnv(["android", "init"], { JAVA_HOME }, io).JAVA_HOME).toBe("/opt/System Java");
  expect(io.run.mock.calls[0]?.[0]).toBe("java");
});

test.each(["17", "25", "26"])("accepts Java %s", (major) => {
  expect(androidJavaEnv(["android"], {}, probe(properties(major))).JAVA_HOME).toBe(
    "/opt/System Java",
  );
});

test.each(["27", "16", "1.8"])("rejects unsupported Java %s", (major) => {
  expect(() => androidJavaEnv(["android"], {}, probe(properties(major)))).toThrow(
    /unsupported Java major.*17 through 26/,
  );
});

test.each([
  { args: [] },
  { args: ["dev"] },
  { args: ["build"] },
  { args: ["build", "--config", "android"] },
])("non-Android arguments $args bypass all Java work", ({ args }) => {
  const env = Object.freeze({ JAVA_HOME: "/invalid" });
  const io = probe();
  expect(androidJavaEnv(args, env, io)).toBe(env);
  expect(io.run).not.toHaveBeenCalled();
  expect(io.access).not.toHaveBeenCalled();
});

test("handles Windows executables, CRLF, and spaces", () => {
  const io = probe(properties("26", "C:\\Program Files\\Java\\jdk"));
  io.platform = "win32";
  io.run.mockReturnValue({
    ...properties(),
    stderr: properties("26", "C:\\Program Files\\Java\\jdk").stderr.replaceAll("\n", "\r\n"),
  });
  const env = androidJavaEnv(["android"], {}, io);
  expect(env.JAVA_HOME).toBe("C:\\Program Files\\Java\\jdk");
  expect(io.run.mock.calls[0]?.[0]).toBe("java.exe");
  expect(io.access).toHaveBeenCalledWith(
    "C:\\Program Files\\Java\\jdk\\bin\\javac.exe",
    constants.X_OK,
  );
  androidJavaEnv(["android"], env, io);
  expect(io.run.mock.calls[1]?.[0]).toBe("C:\\Program Files\\Java\\jdk\\bin\\java.exe");
});

test.each([
  "",
  "java.home = /jdk\n",
  "java.specification.version = 26\n",
  properties("26-ea").stderr,
  properties("garbage").stderr,
  properties("1.26").stderr,
  properties("26", "relative/jdk").stderr,
  properties().stderr + "java.home = /other\n",
])("rejects missing or malformed properties %#", (stderr) => {
  expect(() => androidJavaEnv(["android"], {}, probe({ status: 0, stderr, stdout: "" }))).toThrow(
    /missing or malformed/,
  );
});

test.each([
  { status: 1 },
  { status: null, error: new Error("secret ENOENT") },
  { status: null, error: new Error("secret ETIMEDOUT"), signal: "SIGTERM" },
])("fails safely for missing/unusable/timed-out java %#", (result) => {
  const io = probe();
  io.run.mockReturnValue({ ...result, stderr: "secret", stdout: "secret" });
  expect(() => androidJavaEnv(["android"], { JAVA_HOME: "/secret" }, io)).toThrow(
    /^Android Java \(JAVA_HOME\): java is missing, unusable, or timed out\./,
  );
  expect(io.run).toHaveBeenCalledTimes(1);
  expect(io.access).not.toHaveBeenCalled();
});

test("sanitizes thrown probe failures", () => {
  const io = probe();
  io.run.mockImplementation(() => {
    throw new Error("secret");
  });
  expect(() => androidJavaEnv(["android"], {}, io)).toThrow(/could not run java/);
});

test("also accepts properties on stdout", () => {
  const io = probe({ status: 0, stderr: "", stdout: properties().stderr });
  expect(androidJavaEnv(["android"], {}, io).JAVA_HOME).toBe("/opt/System Java");
});

test("rejects a JRE or non-executable javac without leaking errors", () => {
  const io = probe();
  io.access.mockImplementation(() => {
    throw new Error("secret");
  });
  expect(() => androidJavaEnv(["android"], {}, io)).toThrow(
    /JDK compiler javac is missing or not executable/,
  );
});

test("Gradle builds against the same NDK the environment selects", () => {
  const gradle = readFileSync(
    new URL("../../src-tauri/gen/android/app/build.gradle.kts", import.meta.url),
    "utf8",
  );
  expect(gradle).toContain(`ndkVersion = "${NDK_VERSION}"`);
});

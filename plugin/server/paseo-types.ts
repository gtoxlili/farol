// Types of the Paseo SDK a plugin's server code receives. Paseo supplies the
// SDK at runtime and installs only production dependencies, so the types come
// from @getpaseo/plugin (which the plugin compiler resolves), not from
// @getpaseo/client.
import type { PluginHandlerContext } from "@getpaseo/plugin/server";

export type PaseoApi = PluginHandlerContext["paseo"];
export type PaseoAgent = NonNullable<ReturnType<ReturnType<PaseoApi["agents"]["ref"]>["current"]>>;

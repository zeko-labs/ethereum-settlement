const target = Bun.env.MINA_ARCHIVE_TARGET
const port = Number(Bun.env.PORT ?? "8083")
if (!target) throw new Error("MINA_ARCHIVE_TARGET is required")
if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) throw new Error("Invalid PORT")

type GraphqlBody = {
	query?: unknown
	variables?: Record<string, unknown> | null
}

const server = Bun.serve({
	hostname: "127.0.0.1",
	port,
	async fetch(request) {
		const url = new URL(request.url)
		if (request.method === "GET" && url.pathname === "/health") {
			return Response.json({ status: "healthy", target })
		}
		if (request.method !== "POST" || url.pathname !== "/graphql") {
			return new Response("Not found", { status: 404 })
		}

		const body = (await request.json()) as GraphqlBody
		const variables = body.variables ?? {}
		for (const name of ["fromActionState", "endActionState"] as const) {
			if (!(name in variables)) variables[name] = null
		}
		const response = await fetch(target, {
			method: "POST",
			headers: { "content-type": "application/json" },
			body: JSON.stringify({ ...body, variables })
		})
		return new Response(response.body, {
			status: response.status,
			headers: response.headers
		})
	}
})

console.log(`Mina archive compatibility proxy listening on ${server.url}`)

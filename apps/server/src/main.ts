import { createServer } from "./server.js"

const port = Number(process.env.PORT ?? 3000)
const server = createServer()

server.listen(port, () => {
	console.log(`AnimaOS Kit server running on http://localhost:${port}`)
	console.log(`API: http://localhost:${port}/api`)
	console.log(`WebSocket: ws://localhost:${port}/ws`)
})

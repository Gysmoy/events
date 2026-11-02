const express = require("express")
const http = require("http")
const socketIo = require("socket.io")
const crypto = require("crypto")

// Solo los servicios de DevEx usan este servidor. Sin secreto no arranca.
const SECRET = process.env.EVENTS_SECRET || ""
const ALLOWED_SERVICES = (process.env.ALLOWED_SERVICES || "devex").split(",").map((s) => s.trim()).filter(Boolean)
const ALLOWED_ORIGINS = (process.env.ALLOWED_ORIGINS || "https://devex.pe").split(",").map((s) => s.trim()).filter(Boolean)

if (!SECRET) {
    console.error("Falta EVENTS_SECRET: el servidor no arranca sin autenticación")
    process.exit(1)
}

function safeEqual(a, b) {
    const left = Buffer.from(String(a))
    const right = Buffer.from(String(b))
    return left.length === right.length && crypto.timingSafeEqual(left, right)
}

// Token de conexión: "<expira en segundos unix>.<hmac sha256 de 'servicio.expira'>".
// Lo firma el backend del servicio para sus usuarios con sesión.
function isValidSocketToken(service, token) {
    const [expires, signature] = String(token || "").split(".")
    if (!/^\d+$/.test(expires) || Number(expires) < Date.now() / 1000) return false
    const expected = crypto.createHmac("sha256", SECRET).update(`${service}.${expires}`).digest("hex")
    return safeEqual(signature, expected)
}

// Emitir y consultar es solo para los backends: requiere el secreto.
function requireSecret(req, res, next) {
    const token = (req.headers.authorization || "").replace(/^Bearer /, "")
    if (!safeEqual(token, SECRET)) {
        return res.status(401).json({ error: "No autorizado" })
    }
    next()
}

const app = express()
const server = http.createServer(app)
const io = socketIo(server, {
    cors: {
        origin: ALLOWED_ORIGINS,
        methods: ["GET", "POST"],
    },
    // Un navegador siempre envía Origin: lo que no venga de un dominio permitido no pasa.
    allowRequest: (req, callback) => callback(null, ALLOWED_ORIGINS.includes(req.headers.origin)),
})

// Middleware
app.use(express.json())

// Almacenar conexiones con sus filtros organizadas por servicio
const connectedClients = new Map() // service -> Map(socketId -> clientData)

// Función para comparar filtros
function filtersMatch(clientFilters, notificationFilters) {
    // Verificar que todos los filtros de la notificación coincidan con los del cliente
    for (const [key, value] of Object.entries(notificationFilters)) {
        if (clientFilters[key] !== value) {
            return false
        }
    }
    return true
}

// Función para obtener clientes que coincidan con los filtros en un servicio específico
function getMatchingClients(service, filters) {
    const matchingClients = []
    const serviceClients = connectedClients.get(service)

    if (!serviceClients) {
        return matchingClients
    }

    for (const [socketId, clientData] of serviceClients.entries()) {
        if (filtersMatch(clientData.filters, filters)) {
            matchingClients.push({
                socketId,
                socket: clientData.socket,
                filters: clientData.filters,
            })
        }
    }

    return matchingClients
}

// Función para inicializar un servicio si no existe
function initializeService(service) {
    if (!connectedClients.has(service)) {
        connectedClients.set(service, new Map())
    }
}

// El namespace raíz no se usa
io.of("/").use((socket, next) => next(new Error("No autorizado")))

// Un namespace por cada servicio permitido; cualquier otro es inválido
const namespaces = io.of(new RegExp(`^/(${ALLOWED_SERVICES.join("|")})$`))

namespaces.use((socket, next) => {
    const service = socket.nsp.name.substring(1)
    if (!isValidSocketToken(service, socket.handshake.auth?.token)) {
        return next(new Error("No autorizado"))
    }
    next()
})

namespaces.on("connection", (socket) => {
    // Extraer el nombre del servicio del namespace
    const service = socket.nsp.name.substring(1) // Remover el '/' inicial

    // Inicializar el servicio si no existe
    initializeService(service)

    // Evento para registrar filtros adicionales del cliente
    socket.on("register_filters", (filters) => {
        // Validar que filters sea un objeto
        if (typeof filters !== "object" || filters === null) {
            socket.emit("error", { message: "Los filtros deben ser un objeto válido" })
            return
        }

        // Los filtros incluyen automáticamente el servicio
        const completeFilters = {
            service: service,
            ...filters,
        }

        // Almacenar la conexión con sus filtros
        const serviceClients = connectedClients.get(service)
        serviceClients.set(socket.id, {
            socket: socket,
            filters: completeFilters,
            connectedAt: new Date(),
        })

        socket.emit("filters_registered", {
            message: "Filtros registrados correctamente",
            service: service,
            filters: completeFilters,
        })
    })

    // Evento para actualizar filtros
    socket.on("update_filters", (newFilters) => {
        if (typeof newFilters !== "object" || newFilters === null) {
            socket.emit("error", { message: "Los filtros deben ser un objeto válido" })
            return
        }

        const serviceClients = connectedClients.get(service)
        if (serviceClients && serviceClients.has(socket.id)) {
            const completeFilters = {
                service: service,
                ...newFilters,
            }

            const clientData = serviceClients.get(socket.id)
            clientData.filters = completeFilters
            serviceClients.set(socket.id, clientData)

            socket.emit("filters_updated", {
                message: "Filtros actualizados correctamente",
                service: service,
                filters: completeFilters,
            })
        }
    })

    // Evento para obtener filtros actuales
    socket.on("get_filters", () => {
        const serviceClients = connectedClients.get(service)
        const clientData = serviceClients?.get(socket.id)

        if (clientData) {
            socket.emit("current_filters", {
                service: service,
                filters: clientData.filters,
            })
        } else {
            socket.emit("error", { message: "No hay filtros registrados para este cliente" })
        }
    })

    // Manejar desconexión
    socket.on("disconnect", () => {
        const serviceClients = connectedClients.get(service)
        if (serviceClients) {
            serviceClients.delete(socket.id)

            // Si no quedan clientes en el servicio, limpiar el servicio
            if (serviceClients.size === 0) {
                connectedClients.delete(service)
            }
        }
    })
})

// Endpoint REST para enviar eventos
app.post("/emit", requireSecret, (req, res) => {
    const { service, filters, eventType, data } = req.body

    // Validaciones
    if (!service || typeof service !== "string" || !ALLOWED_SERVICES.includes(service)) {
        return res.status(400).json({
            error: "Se requiere un nombre de servicio válido",
        })
    }

    if (!filters || typeof filters !== "object") {
        return res.status(400).json({
            error: "Se requieren filtros válidos (objeto clave-valor)",
        })
    }

    if (!eventType || typeof eventType !== "string") {
        return res.status(400).json({
            error: "Se requiere un identificador de evento (eventType)",
        })
    }

    // Los filtros deben incluir el servicio
    const completeFilters = {
        service: service,
        ...filters,
    }

    // Obtener clientes que coincidan con los filtros en el servicio específico
    const matchingClients = getMatchingClients(service, completeFilters)

    // Enviar evento a clientes que coincidan (solo eventType y data)
    matchingClients.forEach((client) => {
        client.socket.emit(eventType, data || {})
    })

    res.json({
        success: true,
        message: "Evento enviado",
        service: service,
        eventType: eventType,
        clientsNotified: matchingClients.length,
        filters: completeFilters,
    })
})

// Endpoint para obtener estadísticas
app.get("/stats", requireSecret, (req, res) => {
    const stats = {
        totalServices: connectedClients.size,
        totalClients: 0,
        services: {},
    }

    for (const [service, serviceClients] of connectedClients.entries()) {
        stats.totalClients += serviceClients.size
        stats.services[service] = {
            connectedClients: serviceClients.size,
            clients: [],
        }

        for (const [socketId, clientData] of serviceClients.entries()) {
            stats.services[service].clients.push({
                socketId,
                filters: clientData.filters,
                connectedAt: clientData.connectedAt,
            })
        }
    }

    res.json(stats)
})

// Endpoint para obtener clientes por filtros específicos en un servicio
app.post("/clients/filter", requireSecret, (req, res) => {
    const { service, filters } = req.body

    if (!service || typeof service !== "string") {
        return res.status(400).json({
            error: "Se requiere un nombre de servicio válido",
        })
    }

    if (!filters || typeof filters !== "object") {
        return res.status(400).json({
            error: "Se requieren filtros válidos (objeto clave-valor)",
        })
    }

    const completeFilters = {
        service: service,
        ...filters,
    }

    const matchingClients = getMatchingClients(service, completeFilters)

    res.json({
        service: service,
        matchingClients: matchingClients.length,
        clients: matchingClients.map((client) => ({
            socketId: client.socketId,
            filters: client.filters,
        })),
    })
})

// Endpoint de salud
app.get("/health", (req, res) => {
    res.json({ status: "OK" })
})

const PORT = process.env.PORT || 3000

server.listen(PORT, () => {
    console.log(`Servidor WebSocket ejecutándose en puerto ${PORT}`)
    console.log(`Servicios permitidos: ${ALLOWED_SERVICES.join(", ")}`)
    console.log(`Orígenes permitidos: ${ALLOWED_ORIGINS.join(", ")}`)
})

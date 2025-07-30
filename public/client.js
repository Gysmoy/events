let ws = null
let isConnected = false

// Elementos del DOM
const connectBtn = document.getElementById("connectBtn")
const connectionStatus = document.getElementById("connectionStatus")
const serverUrl = document.getElementById("serverUrl")
const currentFilters = document.getElementById("currentFilters")
const log = document.getElementById("log")

// Función para agregar entrada al log
function addLogEntry(message, type = "info") {
    const timestamp = new Date().toLocaleTimeString()
    const entry = document.createElement("div")
    entry.className = `log-entry ${type}`
    entry.textContent = `[${timestamp}] ${message}`
    log.appendChild(entry)
    log.scrollTop = log.scrollHeight
}

// Función para actualizar estado de conexión
function updateConnectionStatus(connected) {
    isConnected = connected
    connectionStatus.textContent = connected ? "Conectado" : "Desconectado"
    connectionStatus.className = `status ${connected ? "connected" : "disconnected"}`
    connectBtn.textContent = connected ? "Desconectar" : "Conectar"
}

// Función para conectar/desconectar
function toggleConnection() {
    if (isConnected) {
        disconnect()
    } else {
        connect()
    }
}

// Función para conectar
function connect() {
    const url = serverUrl.value
    addLogEntry(`Intentando conectar a ${url}...`)

    try {
        ws = new WebSocket(url)

        ws.onopen = (event) => {
            addLogEntry("✅ Conectado al servidor", "success")
            updateConnectionStatus(true)
        }

        ws.onmessage = (event) => {
            try {
                const message = JSON.parse(event.data)
                handleMessage(message)
            } catch (error) {
                addLogEntry(`❌ Error parseando mensaje: ${error.message}`, "error")
            }
        }

        ws.onclose = (event) => {
            addLogEntry(`🔌 Conexión cerrada (código: ${event.code})`, "info")
            updateConnectionStatus(false)
        }

        ws.onerror = (error) => {
            addLogEntry(`❌ Error de conexión: ${error.message || "Error desconocido"}`, "error")
            updateConnectionStatus(false)
        }
    } catch (error) {
        addLogEntry(`❌ Error creando conexión: ${error.message}`, "error")
    }
}

// Función para desconectar
function disconnect() {
    if (ws) {
        ws.close()
        ws = null
    }
    updateConnectionStatus(false)
}

// Función para manejar mensajes del servidor
function handleMessage(message) {
    const { type, data, timestamp } = message

    switch (type) {
        case "connected":
            addLogEntry(`🆔 ID de cliente: ${data.clientId}`, "success")
            break

        case "filters_registered":
            addLogEntry(`✅ Filtros registrados: ${JSON.stringify(data.filters)}`, "success")
            updateCurrentFilters(data.filters)
            break

        case "filters_updated":
            addLogEntry(`🔄 Filtros actualizados: ${JSON.stringify(data.filters)}`, "success")
            updateCurrentFilters(data.filters)
            break

        case "current_filters":
            updateCurrentFilters(data.filters)
            break

        case "notification":
            addLogEntry(`📢 NOTIFICACIÓN: ${data.message}`, "notification")
            addLogEntry(`   Filtros: ${JSON.stringify(data.filters)}`, "notification")
            if (data.data && Object.keys(data.data).length > 0) {
                addLogEntry(`   Datos: ${JSON.stringify(data.data)}`, "notification")
            }
            break

        case "error":
            addLogEntry(`❌ Error: ${data.message}`, "error")
            break

        case "pong":
            addLogEntry(`🏓 Pong recibido`, "info")
            break

        default:
            addLogEntry(`📨 Mensaje desconocido: ${type}`, "info")
    }
}

// Función para actualizar filtros mostrados
function updateCurrentFilters(filters) {
    if (filters && Object.keys(filters).length > 0) {
        currentFilters.textContent = JSON.stringify(filters, null, 2)
    } else {
        currentFilters.textContent = "Ninguno"
    }
}

// Función para enviar mensaje al servidor
function sendMessage(type, data) {
    if (!ws || ws.readyState !== WebSocket.OPEN) {
        addLogEntry("❌ No hay conexión activa", "error")
        return false
    }

    try {
        ws.send(JSON.stringify({ type, data }))
        return true
    } catch (error) {
        addLogEntry(`❌ Error enviando mensaje: ${error.message}`, "error")
        return false
    }
}

// Función para agregar input de filtro
function addFilterInput() {
    const container = document.getElementById("filterInputs")
    const div = document.createElement("div")
    div.className = "filter-input"
    div.innerHTML = `
        <input type="text" placeholder="Clave" class="filter-key">
        <input type="text" placeholder="Valor" class="filter-value">
        <button onclick="removeFilterInput(this)">❌</button>
    `
    container.appendChild(div)
}

// Función para remover input de filtro
function removeFilterInput(button) {
    const container = document.getElementById("filterInputs")
    if (container.children.length > 1) {
        button.parentElement.remove()
    }
}

// Función para registrar filtros
function registerFilters() {
    const filterInputs = document.querySelectorAll(".filter-input")
    const filters = {}

    filterInputs.forEach((input) => {
        const key = input.querySelector(".filter-key").value.trim()
        const value = input.querySelector(".filter-value").value.trim()

        if (key && value) {
            // Intentar convertir a número si es posible
            const numValue = Number(value)
            filters[key] = isNaN(numValue) ? value : numValue
        }
    })

    if (Object.keys(filters).length === 0) {
        addLogEntry("❌ No hay filtros válidos para registrar", "error")
        return
    }

    addLogEntry(`📝 Registrando filtros: ${JSON.stringify(filters)}`, "info")
    sendMessage("register_filters", filters)
}

// Función para enviar notificación via API
async function sendNotification() {
    const filtersText = document.getElementById("notifyFilters").value.trim()
    const message = document.getElementById("notifyMessage").value.trim()
    const dataText = document.getElementById("notifyData").value.trim()

    if (!filtersText || !message) {
        addLogEntry("❌ Se requieren filtros y mensaje", "error")
        return
    }

    try {
        const filters = JSON.parse(filtersText)
        const data = dataText ? JSON.parse(dataText) : {}

        const response = await fetch("/notify", {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
            },
            body: JSON.stringify({ filters, message, data }),
        })

        const result = await response.json()

        if (response.ok) {
            addLogEntry(`✅ Notificación enviada a ${result.clientsNotified} clientes`, "success")
        } else {
            addLogEntry(`❌ Error enviando notificación: ${result.error}`, "error")
        }
    } catch (error) {
        addLogEntry(`❌ Error: ${error.message}`, "error")
    }
}

// Función para limpiar log
function clearLog() {
    log.innerHTML = ""
}

// Función para enviar ping
function sendPing() {
    sendMessage("ping", {})
}

// Inicialización
document.addEventListener("DOMContentLoaded", () => {
    addLogEntry("🚀 Cliente WebSocket iniciado", "info")

    // Agregar algunos filtros de ejemplo
    document.querySelector(".filter-key").value = "business_id"
    document.querySelector(".filter-value").value = "1"

    // Agregar segundo filtro
    addFilterInput()
    const inputs = document.querySelectorAll(".filter-input")
    const lastInput = inputs[inputs.length - 1]
    lastInput.querySelector(".filter-key").value = "service_id"
    lastInput.querySelector(".filter-value").value = "2"

    // Valores de ejemplo para notificación
    document.getElementById("notifyFilters").value = '{"business_id": 1, "service_id": 2}'
    document.getElementById("notifyMessage").value = "Nuevo pedido recibido"
    document.getElementById("notifyData").value = '{"orderId": 12345, "amount": 99.99}'
})

// Agregar función global para ping (útil para debugging)
window.sendPing = sendPing

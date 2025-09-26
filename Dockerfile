FROM node:20

# Establecer directorio de trabajo
WORKDIR /app

# Copiar package.json y package-lock.json (si existe)
COPY package*.json ./

# Instalar dependencias
RUN npm install 

# Copiar el código fuente
COPY . .



# Exponer el puerto
EXPOSE 3001

# Comando para ejecutar la aplicación
CMD ["node", "server.js"]


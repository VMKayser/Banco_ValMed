// Banco ValMed — web app sin framework. Rutas con hash (#/inicio…) para que
// funcione igual en la raíz o detrás de un prefijo (/banco/). Todas las URL de la
// API son relativas.
'use strict';

const $ = (sel, raiz = document) => raiz.querySelector(sel);
const esc = (v) =>
  String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

const fmtNumero = new Intl.NumberFormat('es-BO', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const bs = (monto) => `Bs ${fmtNumero.format(Number(monto))}`;
const fmtFecha = new Intl.DateTimeFormat('es-BO', { dateStyle: 'medium', timeStyle: 'short', timeZone: 'America/La_Paz' });
const fecha = (iso) => (iso ? fmtFecha.format(new Date(iso)) : '');
const centavos = (m) => Math.round(Number(m) * 100);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const ESTADOS = {
  ACTIVE: 'Esperando pago',
  PAID: 'Pagado',
  EXPIRED: 'Vencido',
  CANCELLED: 'Anulado',
  REFUNDED: 'Devuelto',
};

// sessionStorage puede fallar (modo privado, sitio bloqueado): la app sigue sin él.
const almacen = {
  leer(k) { try { return sessionStorage.getItem(k); } catch { return null; } },
  guardar(k, v) {
    try { if (v == null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch { /* sin almacenamiento */ }
  },
};

const estado = {
  token: almacen.leer('vm.token'),
  perfil: null,
  config: null,
  recibo: null,
  pendiente: null, // id de un QR abierto con la cámara del celular antes de entrar
  limpiezas: [],
};

const icono = {
  escanear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3M4 12h16"/></svg>',
  qr: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><path d="M14 14h2v2h-2zM18 18h2v2h-2zM14 18h2M18 14h2"/></svg>',
  mas: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="M12 8v8M8 12h8"/></svg>',
  lista: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01"/></svg>',
  codigo: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m8 8-4 4 4 4M16 8l4 4-4 4M13.5 5l-3 14"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m5 12.5 4.5 4.5L19 7.5"/></svg>',
};

// ── API ──────────────────────────────────────────────────────────────────────
class ErrorApi extends Error {
  constructor(status, cuerpo) {
    super(cuerpo?.message || `Error ${status}`);
    this.status = status;
    this.code = cuerpo?.code;
    this.details = cuerpo?.details || [];
  }
}

async function api(ruta, { metodo = 'GET', cuerpo, sinSesion = false } = {}) {
  const cabeceras = { Accept: 'application/json' };
  if (cuerpo !== undefined) cabeceras['Content-Type'] = 'application/json';
  if (estado.token && !sinSesion) cabeceras.Authorization = `Bearer ${estado.token}`;
  let respuesta;
  try {
    respuesta = await fetch(ruta, {
      method: metodo,
      headers: cabeceras,
      body: cuerpo === undefined ? undefined : JSON.stringify(cuerpo),
    });
  } catch {
    throw new ErrorApi(0, { code: 'NETWORK', message: 'No hay conexión con el banco' });
  }
  if (respuesta.status === 204) return null;
  const datos = await respuesta.json().catch(() => null);
  if (!respuesta.ok) {
    if (respuesta.status === 401 && datos?.code === 'UNAUTHENTICATED' && estado.token) {
      olvidarSesion();
      ir('#/entrar');
      setTimeout(() => aviso('Tu sesión venció. Vuelve a entrar.', 'error'), 50);
    }
    throw new ErrorApi(respuesta.status, datos);
  }
  return datos;
}

function textoError(e) {
  if (e instanceof ErrorApi && e.details.length) {
    return `${e.message}: ${e.details.map((d) => `${d.field} (${d.issue})`).join(', ')}`;
  }
  return e?.message || 'Algo salió mal';
}

// ── Avisos ───────────────────────────────────────────────────────────────────
let temporizadorAviso;
function aviso(texto, tipo = 'ok') {
  const caja = $('#aviso');
  caja.textContent = texto;
  caja.className = `aviso ${tipo === 'error' ? 'error' : ''}`;
  caja.hidden = false;
  clearTimeout(temporizadorAviso);
  if (tipo !== 'error') temporizadorAviso = setTimeout(() => (caja.hidden = true), 4000);
}
function limpiarAviso() {
  clearTimeout(temporizadorAviso);
  $('#aviso').hidden = true;
}
const mostrarError = (e) => aviso(textoError(e), 'error');

// ── Sesión ───────────────────────────────────────────────────────────────────
function guardarSesion(sesion) {
  estado.token = sesion.token;
  estado.perfil = sesion.profile;
  almacen.guardar('vm.token', sesion.token);
}
function olvidarSesion() {
  estado.token = null;
  estado.perfil = null;
  almacen.guardar('vm.token', null);
}
function entrarCon(sesion) {
  guardarSesion(sesion);
  const pendiente = estado.pendiente;
  estado.pendiente = null;
  ir(pendiente ? `#/confirmar/${pendiente}` : '#/inicio');
}

// ── Utilidades de pantalla ───────────────────────────────────────────────────
function pintar(html) {
  $('#vista').innerHTML = html;
}
function cabecera(titulo, volver = '#/inicio') {
  return `<header class="cab-vista"><a href="${volver}" class="volver" aria-label="Volver">‹</a><h1>${esc(titulo)}</h1></header>`;
}
function alLimpiar(fn) {
  estado.limpiezas.push(fn);
}

// Envía un formulario: junta los campos, deshabilita el botón y muestra errores.
function formulario(sel, accion) {
  const f = $(sel);
  f.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const boton = f.querySelector('button[type="submit"], button:not([type])');
    const datos = Object.fromEntries(new FormData(f).entries());
    for (const k of Object.keys(datos)) if (typeof datos[k] === 'string') datos[k] = datos[k].trim();
    if (boton) boton.disabled = true;
    try {
      await accion(datos, f);
    } catch (e) {
      mostrarError(e);
    } finally {
      if (boton && boton.isConnected) boton.disabled = false;
    }
  });
}

function cuentaAtras(el, iso, alVencer) {
  const fin = new Date(iso).getTime();
  let t;
  const tick = () => {
    const s = Math.max(0, Math.round((fin - Date.now()) / 1000));
    const h = Math.floor(s / 3600);
    const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
    const ss = String(s % 60).padStart(2, '0');
    if (el.isConnected) el.textContent = h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
    if (s === 0) {
      clearInterval(t);
      alVencer?.();
    }
  };
  t = setInterval(tick, 1000);
  tick();
  return () => clearInterval(t);
}

async function copiar(texto) {
  try {
    await navigator.clipboard.writeText(texto);
  } catch {
    const area = document.createElement('textarea');
    area.value = texto;
    area.setAttribute('readonly', '');
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.append(area);
    area.select();
    document.execCommand('copy');
    area.remove();
  }
  aviso('Copiado');
}

function delegarCopiar(raiz) {
  raiz.addEventListener('click', (ev) => {
    const b = ev.target.closest('[data-copiar]');
    if (b) copiar(b.dataset.copiar);
  });
}

function listaMovimientos(movs) {
  if (!movs.length) return '<p class="vacio">Todavía no hay movimientos.</p>';
  return `<ul class="movs">${movs
    .map((m) => {
      const entra = Number(m.amount) > 0;
      const contra = m.counterparty ? ` · ${esc(m.counterparty)}` : '';
      const ref = m.bankReference ? ` · ${esc(m.bankReference)}` : '';
      return `<li>
        <span class="desc">${esc(m.description)}</span>
        <span class="importe ${entra ? 'entra' : 'sale'}">${entra ? '+' : '−'}${bs(Math.abs(Number(m.amount)))}</span>
        <span class="meta">${esc(fecha(m.createdAt))}${contra}${ref}</span>
        <span class="saldo-tras">Saldo ${bs(m.balanceAfter)}</span>
      </li>`;
    })
    .join('')}</ul>`;
}

// Interpreta lo que leyó la cámara o escribió la persona.
function interpretar(texto) {
  const t = String(texto).trim();
  const enUrl = t.match(/[?&#/]p[=/]([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})/i);
  if (enUrl) return { id: enUrl[1] };
  if (UUID.test(t)) return { id: t };
  const codigo = t.replace(/[\s-]/g, '').toUpperCase();
  if (/^[0-9A-Z]{8}$/.test(codigo)) return { codigo };
  return null;
}

async function irAlCobro(lectura) {
  if (lectura.id) return ir(`#/confirmar/${lectura.id}`);
  const encontrado = await api(`api/v1/app/qr-codes/${encodeURIComponent(lectura.codigo)}`);
  ir(`#/confirmar/${encontrado.id}`);
}

// ── Cámara ───────────────────────────────────────────────────────────────────
async function crearDetector() {
  if ('BarcodeDetector' in window) {
    try {
      const formatos = await window.BarcodeDetector.getSupportedFormats();
      if (formatos.includes('qr_code')) {
        const d = new window.BarcodeDetector({ formats: ['qr_code'] });
        return async (fuente) => (await d.detect(fuente))[0]?.rawValue || null;
      }
    } catch { /* se usa jsQR */ }
  }
  const lienzo = document.createElement('canvas');
  const ctx = lienzo.getContext('2d', { willReadFrequently: true });
  return async (fuente) => {
    if (!window.jsQR) return null;
    const ancho = fuente.videoWidth || fuente.naturalWidth || fuente.width;
    const alto = fuente.videoHeight || fuente.naturalHeight || fuente.height;
    if (!ancho || !alto) return null;
    const escala = Math.min(1, 800 / Math.max(ancho, alto));
    lienzo.width = Math.round(ancho * escala);
    lienzo.height = Math.round(alto * escala);
    ctx.drawImage(fuente, 0, 0, lienzo.width, lienzo.height);
    const img = ctx.getImageData(0, 0, lienzo.width, lienzo.height);
    return window.jsQR(img.data, img.width, img.height, { inversionAttempts: 'attemptBoth' })?.data || null;
  };
}

async function iniciarCamara(video, alLeer) {
  if (!window.isSecureContext) throw new Error('La cámara necesita HTTPS (o localhost). Usa el código o sube una foto.');
  if (!navigator.mediaDevices?.getUserMedia) throw new Error('Este navegador no da acceso a la cámara.');
  let flujo;
  try {
    flujo = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' }, audio: false });
  } catch (e) {
    if (e?.name === 'NotAllowedError') throw new Error('No diste permiso para usar la cámara.');
    if (e?.name === 'NotFoundError') throw new Error('No se encontró una cámara.');
    throw new Error('No se pudo abrir la cámara.');
  }
  video.srcObject = flujo;
  await video.play().catch(() => {});
  const detectar = await crearDetector();
  let activo = true;
  const detener = () => {
    activo = false;
    flujo.getTracks().forEach((t) => t.stop());
  };
  const ciclo = async () => {
    if (!activo) return;
    if (video.readyState >= 2) {
      try {
        const texto = await detectar(video);
        if (texto && alLeer(texto)) return detener();
      } catch { /* cuadro ilegible */ }
    }
    setTimeout(ciclo, 180);
  };
  ciclo();
  return detener;
}

async function leerFoto(archivo) {
  const url = URL.createObjectURL(archivo);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const detectar = await crearDetector();
    return await detectar(img);
  } finally {
    URL.revokeObjectURL(url);
  }
}

// ── Pantallas ────────────────────────────────────────────────────────────────
function vistaEntrar() {
  const demo = estado.config?.demoAccounts
    ? `<div class="demo"><p>Cuentas de prueba (PIN <strong>123456</strong>):</p><div class="chips">
        ${[['7000001', 'Rosa'], ['7000002', 'Juan · Bs 50'], ['7000003', 'Luis'], ['7000004', 'Carla']]
          .map(([ci, n]) => `<button type="button" data-ci="${ci}">${ci} ${esc(n)}</button>`)
          .join('')}</div></div>`
    : '';
  pintar(`
    <section class="tarjeta">
      <h1>Entra a tu cuenta</h1>
      <p class="sub">Banco de práctica: el dinero no es real.</p>
      <form id="f" novalidate>
        <div class="fila">
          <label>C.I.<input name="ci" inputmode="numeric" autocomplete="username" maxlength="10" required></label>
          <label>Compl.<input name="ciComplement" maxlength="2" autocapitalize="characters" placeholder="opc."></label>
        </div>
        <label>PIN de 6 dígitos<input name="pin" type="password" inputmode="numeric" autocomplete="current-password" maxlength="6" required></label>
        <button class="btn primario" type="submit">Entrar</button>
      </form>
      <p class="centro"><a href="#/abrir">Abrir una cuenta nueva</a></p>
      ${demo}
    </section>`);
  $('.demo')?.addEventListener('click', (ev) => {
    const b = ev.target.closest('[data-ci]');
    if (!b) return;
    $('#f [name="ci"]').value = b.dataset.ci;
    $('#f [name="pin"]').value = '123456';
  });
  formulario('#f', async (d) => {
    const sesion = await api('api/v1/app/sessions', {
      metodo: 'POST',
      sinSesion: true,
      cuerpo: { ci: d.ci, ciComplement: d.ciComplement || null, pin: d.pin },
    });
    entrarCon(sesion);
  });
}

function vistaAbrir() {
  const saldo = estado.config ? bs(estado.config.initialBalance) : 'saldo simulado';
  pintar(`
    ${cabecera('Abrir una cuenta', '#/entrar')}
    <section class="tarjeta">
      <p class="sub">Tu cuenta nace con <strong>${saldo}</strong> de mentira para probar pagos.</p>
      <form id="f" novalidate>
        <div class="fila">
          <label>C.I.<input name="ci" inputmode="numeric" maxlength="10" required></label>
          <label>Compl.<input name="ciComplement" maxlength="2" autocapitalize="characters" placeholder="opc."></label>
        </div>
        <label>Nombre completo<input name="fullName" autocomplete="name" maxlength="120" required></label>
        <label>PIN de 6 dígitos<input name="pin" type="password" inputmode="numeric" autocomplete="new-password" maxlength="6" required></label>
        <label>Repite el PIN<input name="pin2" type="password" inputmode="numeric" autocomplete="new-password" maxlength="6" required></label>
        <button class="btn primario" type="submit">Abrir cuenta</button>
      </form>
    </section>`);
  formulario('#f', async (d) => {
    if (d.pin !== d.pin2) throw new Error('Los dos PIN no coinciden');
    const sesion = await api('api/v1/app/customers', {
      metodo: 'POST',
      sinSesion: true,
      cuerpo: { ci: d.ci, ciComplement: d.ciComplement || null, fullName: d.fullName, pin: d.pin },
    });
    aviso('Cuenta abierta');
    entrarCon(sesion);
  });
}

async function vistaInicio() {
  pintar('<p class="cargando">Cargando…</p>');
  const [perfil, movs] = await Promise.all([api('api/v1/app/me'), api('api/v1/app/movements?limit=5')]);
  estado.perfil = perfil;
  const nombre = perfil.fullName.split(' ')[0];
  const oculto = almacen.leer('vm.ocultarSaldo') === '1';
  pintar(`
    <section class="saldo">
      <button class="ojo" type="button" id="ojo">${oculto ? 'Mostrar' : 'Ocultar'}</button>
      <p class="saludo">Hola, ${esc(nombre)}</p>
      <p class="etq">Saldo disponible</p>
      <p class="monto" id="monto">${oculto ? 'Bs ••••' : bs(perfil.account.balance)}</p>
      <p class="cuenta">Cuenta ${esc(perfil.account.number)} · C.I. ${esc(perfil.ci)}</p>
    </section>
    <nav class="acciones" aria-label="Acciones">
      <a href="#/pagar">${icono.escanear}<span>Pagar QR</span></a>
      <a href="#/cobrar">${icono.qr}<span>Cobrar</span></a>
      <a href="#/depositar">${icono.mas}<span>Depositar</span></a>
      <a href="#/movimientos">${icono.lista}<span>Movimientos</span></a>
    </nav>
    <section class="tarjeta">
      <div class="titulo-fila"><h2>Últimos movimientos</h2><a href="#/movimientos">Ver todos</a></div>
      ${listaMovimientos(movs)}
    </section>
    <a class="fila-enlace" href="#/desarrolladores">${icono.codigo}
      <span><strong>Desarrolladores</strong><small>Claves de API para cobrar con QR desde otro sistema</small></span><b>›</b>
    </a>`);
  $('#ojo').addEventListener('click', () => {
    const ahora = almacen.leer('vm.ocultarSaldo') !== '1';
    almacen.guardar('vm.ocultarSaldo', ahora ? '1' : null);
    $('#monto').textContent = ahora ? 'Bs ••••' : bs(perfil.account.balance);
    $('#ojo').textContent = ahora ? 'Mostrar' : 'Ocultar';
  });
}

async function vistaPagar() {
  pintar(`
    ${cabecera('Pagar con QR')}
    <div class="visor" id="visor">
      <video id="video" playsinline muted></video>
      <div class="marco"></div>
      <p class="estado-camara" id="estado-camara">Abriendo la cámara…</p>
    </div>
    <section class="tarjeta">
      <form id="fc" class="fila-codigo">
        <label>¿Sin cámara? Escribe el código<input name="codigo" placeholder="7K2Q-M9XD" maxlength="40" autocapitalize="characters" autocomplete="off" required></label>
        <button class="btn primario" type="submit">Buscar</button>
      </form>
      <label class="btn secundario archivo">Subir foto del QR<input type="file" id="foto" accept="image/*" hidden></label>
    </section>`);

  let leyendo = false;
  const alLeer = (texto) => {
    const lectura = interpretar(texto);
    if (!lectura) {
      aviso('Ese QR no es un cobro de Banco ValMed', 'error');
      return false;
    }
    if (leyendo) return true;
    leyendo = true;
    irAlCobro(lectura).catch((e) => {
      leyendo = false;
      mostrarError(e);
    });
    return true;
  };

  formulario('#fc', async (d) => {
    const lectura = interpretar(d.codigo);
    if (!lectura) throw new Error('El código tiene 8 letras o números, como 7K2Q-M9XD');
    await irAlCobro(lectura);
  });
  $('#foto').addEventListener('change', async (ev) => {
    const archivo = ev.target.files?.[0];
    if (!archivo) return;
    try {
      const texto = await leerFoto(archivo);
      if (!texto) throw new Error('No se encontró un QR en la foto');
      alLeer(texto);
    } catch (e) {
      mostrarError(e);
    }
  });

  try {
    const detener = await iniciarCamara($('#video'), alLeer);
    if (!$('#video')) return detener(); // ya se cambió de pantalla
    alLimpiar(detener);
    $('#estado-camara').textContent = 'Apunta al QR del cobro';
  } catch (e) {
    $('#visor')?.classList.add('sin-camara');
    const el = $('#estado-camara');
    if (el) el.textContent = e.message;
  }
}

async function vistaConfirmar(id) {
  pintar('<p class="cargando">Buscando el cobro…</p>');
  const [cobro, perfil] = await Promise.all([api(`api/v1/public/qr/${id}`), api('api/v1/app/me')]);
  estado.perfil = perfil;

  if (cobro.status !== 'ACTIVE') {
    pintar(`
      ${cabecera('Cobro QR', '#/pagar')}
      <section class="tarjeta centro">
        <p><span class="chip ${cobro.status}">${ESTADOS[cobro.status]}</span></p>
        <p class="destino">${esc(cobro.receiverName)}</p>
        <p class="glosa">${esc(cobro.description)}</p>
        <p class="monto-grande">${bs(cobro.amount)}</p>
        <p class="nota">Este cobro ya no se puede pagar.</p>
        <a class="btn primario" href="#/inicio">Volver al inicio</a>
      </section>`);
    return;
  }

  const despues = (centavos(perfil.account.balance) - centavos(cobro.amount)) / 100;
  const alcanza = despues >= 0;
  pintar(`
    ${cabecera('Confirmar pago', '#/pagar')}
    <section class="tarjeta">
      <p class="etq">Pagas a</p>
      <p class="destino">${esc(cobro.receiverName)}</p>
      <p class="glosa">${esc(cobro.description)}</p>
      <p class="monto-grande">${bs(cobro.amount)}</p>
      <dl class="datos">
        <div><dt>Desde</dt><dd>Cuenta ${esc(perfil.account.number)}</dd></div>
        <div><dt>Saldo después</dt><dd>${alcanza ? bs(despues) : '—'}</dd></div>
        <div><dt>Código</dt><dd class="mono">${esc(cobro.code)}</dd></div>
        <div><dt>Vence en</dt><dd id="vence"></dd></div>
      </dl>
      ${alcanza ? '' : `<p class="nota error">Tu saldo (${bs(perfil.account.balance)}) no alcanza. <a href="#/depositar">Depositar</a></p>`}
      <form id="fp" novalidate>
        <label>PIN para confirmar<input name="pin" type="password" inputmode="numeric" maxlength="6" autocomplete="off" required></label>
        <div class="botones">
          <a class="btn secundario" href="#/inicio">Cancelar</a>
          <button class="btn primario" type="submit">Pagar ${bs(cobro.amount)}</button>
        </div>
      </form>
    </section>`);
  alLimpiar(
    cuentaAtras($('#vence'), cobro.expiresAt, () => {
      const b = $('#fp button[type="submit"]');
      if (b) b.disabled = true;
      aviso('El cobro venció', 'error');
    }),
  );
  formulario('#fp', async (d) => {
    const recibo = await api(`api/v1/app/qr/${id}/payments`, { metodo: 'POST', cuerpo: { pin: d.pin } });
    estado.recibo = recibo;
    ir('#/recibo');
  });
}

function vistaRecibo() {
  const r = estado.recibo;
  if (!r) return ir('#/inicio');
  pintar(`
    <section class="tarjeta exito">
      <div class="check">${icono.check}</div>
      <h1>Pago realizado</h1>
      <p class="monto-grande">${bs(r.amount)}</p>
      <dl class="datos">
        <div><dt>Para</dt><dd>${esc(r.receiver.name)}</dd></div>
        <div><dt>Detalle</dt><dd>${esc(r.description)}</dd></div>
        <div><dt>Referencia</dt><dd class="mono">${esc(r.bankReference)}</dd></div>
        <div><dt>Código</dt><dd class="mono">${esc(r.voucherId)}</dd></div>
        <div><dt>Fecha</dt><dd>${esc(fecha(r.paidAt))}</dd></div>
        <div><dt>Tu saldo</dt><dd>${bs(r.balanceAfter)}</dd></div>
      </dl>
      <a class="btn primario" href="#/inicio">Volver al inicio</a>
    </section>`);
}

function vistaCobrar() {
  pintar(`
    ${cabecera('Cobrar con QR')}
    <section class="tarjeta">
      <p class="sub">Genera un QR que otra persona paga desde su app. Se valida solo: verás el pago en cuanto ocurra.</p>
      <form id="f" novalidate>
        <label>Monto (Bs)<input name="amount" inputmode="decimal" placeholder="35.50" required></label>
        <label>Detalle<input name="description" maxlength="140" placeholder="Almuerzo, entrada, etc." required></label>
        <label>Vence en
          <select name="expiresInSeconds">
            <option value="300">5 minutos</option>
            <option value="600" selected>10 minutos</option>
            <option value="1800">30 minutos</option>
            <option value="3600">1 hora</option>
          </select>
        </label>
        <button class="btn primario" type="submit">Generar QR</button>
      </form>
    </section>`);
  formulario('#f', async (d) => {
    const monto = d.amount.replace(',', '.');
    if (!/^\d+(\.\d{1,2})?$/.test(monto)) throw new Error('Escribe el monto con hasta 2 decimales, como 35.50');
    const cobro = await api('api/v1/app/qr', {
      metodo: 'POST',
      cuerpo: { amount: monto, description: d.description, expiresInSeconds: Number(d.expiresInSeconds) },
    });
    ir(`#/cobro/${cobro.id}`);
  });
}

async function vistaCobro(id) {
  pintar('<p class="cargando">Cargando el cobro…</p>');
  let cobro = await api(`api/v1/app/qr/${id}`);
  let pararReloj = () => {};

  const render = () => {
    pararReloj();
    const c = cobro;
    const pago = c.payment;
    let cuerpo;
    if (c.status === 'ACTIVE') {
      cuerpo = `
        <img class="qr" src="api/v1/public/qr/${c.id}/image.svg" alt="QR para pagar ${esc(bs(c.amount))}">
        <p><span class="codigo">${esc(c.code)}</span></p>
        <p class="monto-grande">${bs(c.amount)}</p>
        <p class="glosa">${esc(c.description)}</p>
        <p class="nota">Vence en <strong id="vence"></strong> · se actualiza solo</p>
        <div class="botones iguales">
          <button class="btn secundario" type="button" data-copiar="${esc(c.payload)}">Copiar enlace</button>
          <button class="btn peligro" type="button" id="anular">Anular</button>
        </div>`;
    } else if (pago) {
      cuerpo = `
        <div class="exito"><div class="check">${icono.check}</div></div>
        <p class="monto-grande">${bs(c.amount)}</p>
        <p class="glosa">${esc(c.description)}</p>
        <dl class="datos">
          <div><dt>Pagó</dt><dd>${esc(pago.payer.name)}</dd></div>
          <div><dt>C.I.</dt><dd>${esc(pago.payer.ci)}</dd></div>
          <div><dt>Cuenta</dt><dd>${esc(pago.payer.account)}</dd></div>
          <div><dt>Referencia</dt><dd class="mono">${esc(pago.bankReference)}</dd></div>
          <div><dt>Código</dt><dd class="mono">${esc(pago.voucherId)}</dd></div>
          <div><dt>QR N°</dt><dd class="mono">${c.number}</dd></div>
          <div><dt>Fecha</dt><dd>${esc(fecha(pago.paidAt))}</dd></div>
          ${pago.refundedAt ? `<div><dt>Devuelto</dt><dd>${esc(fecha(pago.refundedAt))} · ${esc(pago.refundReason)}</dd></div>` : ''}
        </dl>
        ${pago.refundedAt ? '' : '<button class="btn peligro" type="button" id="devolver">Devolver el pago</button>'}`;
    } else {
      cuerpo = `
        <p class="monto-grande">${bs(c.amount)}</p>
        <p class="glosa">${esc(c.description)}</p>
        <p class="nota">Este QR ya no se puede pagar.</p>`;
    }
    pintar(`
      ${cabecera('Cobro QR')}
      <section class="tarjeta cobro">
        <p><span class="chip ${c.status}" id="chip">${ESTADOS[c.status]}</span></p>
        ${cuerpo}
      </section>
      <a class="btn primario" style="width:100%" href="#/cobrar">Nuevo cobro</a>`);

    delegarCopiar($('.cobro'));
    if (c.status === 'ACTIVE') {
      pararReloj = cuentaAtras($('#vence'), c.expiresAt);
      $('#anular').addEventListener('click', async () => {
        if (!confirm('¿Anular este cobro? Ya no se podrá pagar.')) return;
        try {
          cobro = await api(`api/v1/app/qr/${id}/cancel`, { metodo: 'POST' });
          render();
        } catch (e) {
          mostrarError(e);
        }
      });
    }
    $('#devolver')?.addEventListener('click', async () => {
      if (!confirm(`¿Devolver ${bs(c.amount)} a ${pago.payer.name}?`)) return;
      try {
        await api(`api/v1/app/payments/${encodeURIComponent(pago.bankReference)}/refunds`, {
          metodo: 'POST',
          cuerpo: { reason: 'DEVOLUCION' },
        });
        cobro = await api(`api/v1/app/qr/${id}`);
        aviso('Pago devuelto');
        render();
      } catch (e) {
        mostrarError(e);
      }
    });
  };

  render();
  alLimpiar(() => pararReloj());

  if (cobro.status === 'ACTIVE') {
    // El QR «se autovalida»: el banco avisa por SSE cuando cambia de estado.
    const eventos = new EventSource(`api/v1/public/qr/${id}/events`);
    alLimpiar(() => eventos.close());
    eventos.addEventListener('status', async (ev) => {
      const publico = JSON.parse(ev.data);
      if (publico.status === cobro.status) return;
      if (publico.status !== 'ACTIVE') eventos.close();
      try {
        cobro = await api(`api/v1/app/qr/${id}`);
        if (cobro.status === 'PAID') aviso(`¡Te pagaron ${bs(cobro.amount)}!`);
        render();
      } catch (e) {
        mostrarError(e);
      }
    });
  }
}

async function vistaMovimientos() {
  pintar(`${cabecera('Movimientos')}<section class="tarjeta" id="lista"><p class="cargando">Cargando…</p></section>
    <button class="btn secundario" style="width:100%" id="mas" hidden>Ver más</button>`);
  let todos = [];
  const cargar = async () => {
    const antes = todos.length ? `&before=${todos[todos.length - 1].id}` : '';
    const nuevos = await api(`api/v1/app/movements?limit=25${antes}`);
    todos = todos.concat(nuevos);
    $('#lista').innerHTML = listaMovimientos(todos);
    $('#mas').hidden = nuevos.length < 25;
  };
  await cargar();
  $('#mas').addEventListener('click', () => cargar().catch(mostrarError));
}

function vistaDepositar() {
  pintar(`
    ${cabecera('Depositar')}
    <section class="tarjeta">
      <p class="sub">Depósito simulado: aparece saldo en tu cuenta para seguir probando (hasta Bs 5.000 por vez).</p>
      <form id="f" novalidate>
        <div class="montos-rapidos">
          ${[50, 100, 500, 1000].map((m) => `<button type="button" data-monto="${m}">${m}</button>`).join('')}
        </div>
        <label>Monto (Bs)<input name="amount" inputmode="decimal" placeholder="100.00" required></label>
        <button class="btn primario" type="submit">Depositar</button>
      </form>
    </section>`);
  $('.montos-rapidos').addEventListener('click', (ev) => {
    const b = ev.target.closest('[data-monto]');
    if (b) $('#f [name="amount"]').value = b.dataset.monto;
  });
  formulario('#f', async (d) => {
    const monto = d.amount.replace(',', '.');
    if (!/^\d+(\.\d{1,2})?$/.test(monto)) throw new Error('Escribe el monto con hasta 2 decimales');
    const cuenta = await api('api/v1/app/deposits', { metodo: 'POST', cuerpo: { amount: monto } });
    aviso(`Depositado. Saldo: ${bs(cuenta.balance)}`);
    ir('#/inicio');
  });
}

async function vistaDesarrolladores() {
  pintar('<p class="cargando">Cargando…</p>');
  const base = estado.config?.publicUrl || location.origin;
  pintar(`
    ${cabecera('Desarrolladores')}
    <section class="tarjeta">
      <h2>Cobrar desde otro sistema</h2>
      <p class="sub">Con una clave, tu sistema crea cobros QR que cualquier cliente de Banco ValMed paga desde esta app. El dinero entra a tu cuenta y tu sistema se entera al instante.</p>
      <p class="sub"><strong>API Market</strong>: las mismas rutas y campos que la API Market (Pago QR) del BNB; para pasar al banco real cambias la URL y las credenciales. <strong>API ValMed</strong>: extras como eventos en vivo y devoluciones.</p>
      <dl class="datos">
        <div><dt>URL base</dt><dd class="mono">${esc(base)}</dd></div>
        <div><dt>Documentación</dt><dd><a href="docs/" target="_blank" rel="noopener">Swagger</a></dd></div>
      </dl>
    </section>
    <section class="tarjeta">
      <h2>Nueva clave de API</h2>
      <form id="fk" novalidate>
        <label>Nombre<input name="name" maxlength="60" placeholder="Ej.: SmartFila Ventas" required></label>
        <label>URL de notificación (opcional)<input name="webhookUrl" inputmode="url" placeholder="http://mi-sistema:8080/pagos/ReceiveNotification"></label>
        <button class="btn primario" type="submit">Crear clave</button>
      </form>
      <div id="nueva"></div>
    </section>
    <section class="tarjeta"><h2>Mis claves</h2><div id="claves"><p class="cargando">Cargando…</p></div></section>
    <section class="tarjeta"><div class="titulo-fila"><h2>Avisos enviados</h2><button class="btn secundario chico" type="button" id="refrescar">Actualizar</button></div><div id="avisos"></div></section>`);

  const pintarClaves = async () => {
    const claves = await api('api/v1/app/api-keys');
    $('#claves').innerHTML = claves.length
      ? `<ul class="lista-simple">${claves
          .map(
            (k) => `<li>
              <div class="linea"><strong>${esc(k.name)}</strong>${
                k.revokedAt
                  ? '<span class="chip CANCELLED">Revocada</span>'
                  : `<button class="btn peligro chico" type="button" data-revocar="${k.id}">Revocar</button>`
              }</div>
              <small>accountId <span class="mono">${esc(k.accountId)}</span> <button class="btn secundario chico" type="button" data-copiar="${esc(k.accountId)}">Copiar</button></small>
              <small>Clave <span class="mono">${esc(k.prefix)}…</span></small>
              <small>Notificación: ${k.webhookUrl ? `<span class="mono">${esc(k.webhookUrl)}</span>` : 'ninguna'}</small>
            </li>`,
          )
          .join('')}</ul>`
      : '<p class="vacio">Aún no creaste claves.</p>';
  };
  const pintarAvisos = async () => {
    const avisos = await api('api/v1/app/webhook-deliveries?limit=15');
    $('#avisos').innerHTML = avisos.length
      ? `<ul class="lista-simple">${avisos
          .map(
            (a) => `<li>
              <div class="linea"><span><span class="chip ${a.status}">${esc(a.status)}</span> <span class="mono">${a.format === 'API_MARKET' ? 'ReceiveNotification' : esc(a.eventType)}</span></span>
                <button class="btn secundario chico" type="button" data-reenviar="${a.id}">Reenviar</button></div>
              <small>${esc(fecha(a.createdAt))} · ${a.format === 'API_MARKET' ? 'API Market' : 'API ValMed'} · intentos ${a.attempts}${a.lastStatusCode ? ` · HTTP ${a.lastStatusCode}` : ''}${a.lastError ? ` · ${esc(a.lastError)}` : ''}</small>
              <small class="mono">${esc(a.url)}</small>
              ${a.lastResponse ? `<small>Respondió: <span class="mono">${esc(a.lastResponse.slice(0, 120))}</span></small>` : ''}
            </li>`,
          )
          .join('')}</ul>`
      : '<p class="vacio">Sin avisos todavía. Aparecen cuando alguien paga un cobro creado con una clave que tiene URL de notificación.</p>';
  };

  await Promise.all([pintarClaves(), pintarAvisos()]);
  delegarCopiar($('#nueva'));
  delegarCopiar($('#claves'));

  formulario('#fk', async (d, f) => {
    const creada = await api('api/v1/app/api-keys', {
      metodo: 'POST',
      cuerpo: { name: d.name, webhookUrl: d.webhookUrl || null },
    });
    f.reset();
    const ejemplo = `curl -X POST ${base}/ClientAuthentication.API/api/v1/auth/token \\
  -H 'Content-Type: application/json' \\
  -d '{"accountId":"${creada.accountId}","authorizationId":"${creada.authorizationId}"}'`;
    $('#nueva').innerHTML = `
      <div class="secreto">
        <p><strong>Cópialas ahora:</strong> el banco no vuelve a mostrar la clave ni el secreto.</p>
        <label>accountId (API Market)</label>
        <div class="copiable"><code>${esc(creada.accountId)}</code><button class="btn secundario chico" type="button" data-copiar="${esc(creada.accountId)}">Copiar</button></div>
        <label>authorizationId (API Market) = clave de API (cabecera X-Api-Key)</label>
        <div class="copiable"><code>${esc(creada.apiKey)}</code><button class="btn secundario chico" type="button" data-copiar="${esc(creada.apiKey)}">Copiar</button></div>
        <label>Secreto para verificar la firma de los avisos (opcional)</label>
        <div class="copiable"><code>${esc(creada.webhookSecret)}</code><button class="btn secundario chico" type="button" data-copiar="${esc(creada.webhookSecret)}">Copiar</button></div>
        <label>Prueba rápida: pedir el token</label>
        <pre class="ejemplo">${esc(ejemplo)}</pre>
      </div>`;
    await pintarClaves();
  });

  $('#claves').addEventListener('click', async (ev) => {
    const b = ev.target.closest('[data-revocar]');
    if (!b || !confirm('¿Revocar la clave? Deja de funcionar de inmediato.')) return;
    try {
      await api(`api/v1/app/api-keys/${b.dataset.revocar}`, { metodo: 'DELETE' });
      await pintarClaves();
    } catch (e) {
      mostrarError(e);
    }
  });
  $('#avisos').addEventListener('click', async (ev) => {
    const b = ev.target.closest('[data-reenviar]');
    if (!b) return;
    try {
      await api(`api/v1/app/webhook-deliveries/${b.dataset.reenviar}/redeliver`, { metodo: 'POST' });
      aviso('Aviso en cola para reenviarse');
      setTimeout(() => pintarAvisos().catch(() => {}), 1500);
    } catch (e) {
      mostrarError(e);
    }
  });
  $('#refrescar').addEventListener('click', () => pintarAvisos().catch(mostrarError));
}

// ── Rutas ────────────────────────────────────────────────────────────────────
const RUTAS = [
  [/^#\/entrar$/, vistaEntrar, false],
  [/^#\/abrir$/, vistaAbrir, false],
  [/^#\/inicio$/, vistaInicio, true],
  [/^#\/pagar$/, vistaPagar, true],
  [/^#\/confirmar\/([0-9a-f-]{36})$/i, vistaConfirmar, true],
  [/^#\/recibo$/, vistaRecibo, true],
  [/^#\/cobrar$/, vistaCobrar, true],
  [/^#\/cobro\/([0-9a-f-]{36})$/i, vistaCobro, true],
  [/^#\/movimientos$/, vistaMovimientos, true],
  [/^#\/depositar$/, vistaDepositar, true],
  [/^#\/desarrolladores$/, vistaDesarrolladores, true],
];

function ir(hash) {
  if (location.hash === hash) enrutar();
  else location.hash = hash;
}

async function enrutar() {
  for (const fn of estado.limpiezas.splice(0)) {
    try { fn(); } catch { /* nada */ }
  }
  const hash = location.hash || (estado.token ? '#/inicio' : '#/entrar');
  $('#salir').hidden = !estado.token;

  for (const [patron, vista, privada] of RUTAS) {
    const m = hash.match(patron);
    if (!m) continue;
    if (privada && !estado.token) {
      if (hash.startsWith('#/confirmar/')) estado.pendiente = m[1];
      return ir('#/entrar');
    }
    if (!privada && estado.token) return ir('#/inicio');
    try {
      await vista(...m.slice(1));
    } catch (e) {
      mostrarError(e);
    }
    window.scrollTo(0, 0);
    return;
  }
  ir(estado.token ? '#/inicio' : '#/entrar');
}

async function arrancar() {
  // Llegada desde la cámara del celular: <base>/?p=<id>
  const p = new URLSearchParams(location.search).get('p');
  if (p && UUID.test(p)) {
    history.replaceState(null, '', `${location.pathname}#/confirmar/${p}`);
  }

  $('#salir').addEventListener('click', async () => {
    try { await api('api/v1/app/sessions/current', { metodo: 'DELETE' }); } catch { /* igual se sale */ }
    olvidarSesion();
    ir('#/entrar');
  });
  // Al cambiar de pantalla se borra el aviso anterior, salvo justo después de mostrarlo.
  window.addEventListener('hashchange', () => {
    if ($('#aviso').classList.contains('error')) limpiarAviso();
    enrutar();
  });

  try {
    estado.config = await api('api/v1/config', { sinSesion: true });
  } catch { /* la app funciona sin estos datos */ }
  enrutar();
}

arrancar();

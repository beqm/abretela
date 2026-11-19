// Em que sistema o app está rodando, para os textos que mudam de um para o
// outro (no macOS a "bandeja" é a barra de menus).

/** O app está rodando no macOS? */
export const isMac = () => /Macintosh|Mac OS X/.test(navigator.userAgent);

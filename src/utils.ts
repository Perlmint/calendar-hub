export type AsyncReturnType<T extends (...args: any) => any> = T extends (...args: any) => Promise<infer R> ? R : never;

export function formDataToJsonString(data: FormData) {
    const ret: Record<string, string> = {};
    data.forEach((value, key) => {
        ret[key] = value.toString();
    });
    return JSON.stringify(ret);
}

//! Compiled materials (`.vmat_c`): the shader and its named parameters, textures by path.

use crate::Resource;
use crate::kv3::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    pub name: String,
    /// The shader (`csgo_weapon.vfx`, `csgo_complex.vfx`...).
    pub shader: String,
    pub ints: Vec<(String, i64)>,
    pub floats: Vec<(String, f32)>,
    pub vectors: Vec<(String, [f32; 4])>,
    /// Texture parameters → compiled texture paths (`..._color_psd_xxxx.vtex`).
    pub textures: Vec<(String, String)>,
}

impl Material {
    #[must_use]
    pub fn texture(&self, name: &str) -> Option<&str> {
        self.textures
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    #[must_use]
    pub fn int(&self, name: &str) -> Option<i64> {
        self.ints.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    #[must_use]
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    #[must_use]
    pub fn vector(&self, name: &str) -> Option<[f32; 4]> {
        self.vectors
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }
}

fn params<T>(
    data: &Value,
    key: &str,
    value_key: &str,
    read: impl Fn(&Value) -> Option<T>,
) -> Vec<(String, T)> {
    data.get(key)
        .map(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|p| Some((p.str_of("m_name").to_owned(), read(p.get(value_key)?)?)))
        .collect()
}

/// Load a material's parameters.
pub fn load(bytes: &[u8]) -> Result<Material, String> {
    let data = Resource::parse(bytes)?.data_kv3()?;
    Ok(Material {
        name: data.str_of("m_materialName").to_owned(),
        shader: data.str_of("m_shaderName").to_owned(),
        ints: params(&data, "m_intParams", "m_nValue", Value::as_i64),
        floats: params(&data, "m_floatParams", "m_flValue", Value::as_f32),
        vectors: params(&data, "m_vectorParams", "m_value", |v| {
            let f = v.as_f32s();
            (f.len() >= 4).then(|| [f[0], f[1], f[2], f[3]])
        }),
        textures: params(&data, "m_textureParams", "m_pValue", |v| {
            v.as_str().map(str::to_owned)
        }),
    })
}

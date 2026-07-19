use bevy::math::Vec3;
use std::ops::{Add, AddAssign, Mul};

#[derive(Clone, Copy, Default)]
pub struct BiVector3 {
    pub force: Vec3,
    pub torque: Vec3,
}

impl Add for BiVector3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self { force: self.force + rhs.force, torque: self.torque + rhs.torque }
    }
}

impl AddAssign for BiVector3 {
    fn add_assign(&mut self, rhs: Self) {
        self.force += rhs.force;
        self.torque += rhs.torque;
    }
}

impl Mul<f32> for BiVector3 {
    type Output = Self;
    fn mul(self, f: f32) -> Self {
        Self { force: self.force * f, torque: self.torque * f }
    }
}
